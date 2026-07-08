//! STEP (ISO-10303-21) interop — the M15.0 / ADR-070 Leg-A seam: import a real STEP part's B-rep, keep its
//! **faces / edges as referenceable entities**, tessellate the planar subset for wgpu, and re-export — all
//! **behind the M8.5 `Interchange` trait pattern** (the [`CadInterchange`] trait; no foreign STEP-lib type
//! crosses the public surface, invariant 5, CI grep-gated for the future OCCT dep).
//!
//! **Honest scope (the ADR-070 boundary — stated, not papered over).**
//! - This is a **pure-Rust ISO-10303-21 Part-21 reader/writer** for the **planar B-rep + faceted** subset —
//!   the kernel-free exchange that needs no C++. It parses the real ADVANCED_BREP topology chain
//!   (`ADVANCED_FACE → FACE_OUTER_BOUND → EDGE_LOOP → ORIENTED_EDGE → EDGE_CURVE → VERTEX_POINT →
//!   CARTESIAN_POINT`) that CAD tools export, and also faceted `POLY_LOOP` bounds.
//! - **Curved / trimmed-NURBS surfaces are NOT evaluated here** (`CYLINDRICAL_SURFACE`, `B_SPLINE_SURFACE`,
//!   …). They are recorded as **referenceable [`CadFace`]s with an explained [`UnsupportedNote`]** (never a
//!   silent drop, ADR-016) — their exact tessellation rides **OpenCascade (OCCT) FFI, native/server-only,
//!   OUT of the determinism guarantee** (the §3 crate audit; OCCT is C++/non-bit-deterministic and cannot
//!   even be *built* in a no-cmake/no-C++ environment — the seam is real, not hypothetical).
//! - **Re-export is FACETED**, not trimmed-NURBS: we faithfully preserve **geometry** (vertices + planar
//!   faces) within a **declared, measured tolerance budget**; we do **not** round-trip NURBS (that is the
//!   OCCT seam). "STEP import here = display / annotate / exchange, **not** in-engine B-rep *editing*"
//!   (ADR-070; in-engine B-rep editing gates on `truck` maturity — a named future).
//!
//! **Safety (the M10.2 gate, ADR-031).** Every parse is **bounds-checked**: an oversized file, a malformed
//! statement, an unresolved `#ref`, or an entity-count bomb is a **`Blocked`-explained [`StepError`], never a
//! panic**.

use crate::{Units, UnsupportedNote};
use metrocalk_csg::TriMesh;
use std::collections::{BTreeMap, BTreeSet};

/// Reject a STEP text larger than this before parsing (the M10.2 size cap; mirrors `assets::MAX_IMPORT_BYTES`).
pub const MAX_STEP_BYTES: usize = 64 * 1024 * 1024;
/// Reject a file with more entity instances than this (the decode-bomb guard — a Part-21 file can name
/// millions of `#id`s; cap before allocating the graph). A real commercial-CAD assembly with embedded
/// tessellation (the M15.7 case) legitimately has millions of entities, so the cap is generous but bounded.
pub const MAX_ENTITIES: usize = 30_000_000;

/// What kind of surface underlies a [`CadFace`] — planar faces are tessellated here; everything else is a
/// referenceable face whose exact tessellation is the OCCT seam (an explained note is emitted).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FaceKind {
    /// A `PLANE` surface — tessellated exactly (fan-triangulated) by this crate.
    Planar,
    /// A curved / freeform surface (`CYLINDRICAL_SURFACE`, `B_SPLINE_SURFACE`, …) — referenced but NOT
    /// tessellated here; the exact tessellation is the OCCT native/server seam.
    Curved,
}

/// A referenceable edge — the STEP `EDGE_CURVE` #id + its endpoints. The hook M15.3 PMI/GD&T attaches to.
#[derive(Clone, PartialEq, Debug)]
pub struct CadEdge {
    /// The STEP entity id (`#id`) — a stable, referenceable handle.
    pub id: u64,
    /// The two endpoints, in world coordinates (scene units).
    pub ends: [[f64; 3]; 2],
}

/// A referenceable face — the STEP `ADVANCED_FACE`/`FACE` #id + its boundary polygon + its edges. The
/// primary hook M15.3 semantic-PMI (a feature-control-frame on a face) attaches to.
#[derive(Clone, PartialEq, Debug)]
pub struct CadFace {
    /// The STEP entity id (`#id`) — a stable, referenceable handle.
    pub id: u64,
    /// Planar (tessellated here) or curved (OCCT seam).
    pub kind: FaceKind,
    /// The outer-boundary polygon, ordered (world coordinates).
    pub outer: Vec<[f64; 3]>,
    /// The face's referenceable edges.
    pub edges: Vec<CadEdge>,
    /// M15.8 (ADR-078) — the recognized **analytic** surface under a `Curved` face (cylinder / cone /
    /// sphere / torus), tessellated closed-form + deterministic by this crate. `None` for planar faces and
    /// for NURBS/freeform (which stay the licensed-kernel/OCCT seam — never hand-rolled).
    pub surface: Option<crate::analytic::AnalyticSurface>,
    /// The `ADVANCED_FACE` `same_sense` flag — whether the face's outward side agrees with the surface's
    /// positive normal (a bore's cylinder wall faces INWARD → `.F.`). Drives exact analytic orientation.
    pub same_sense: bool,
}

/// One solid body (a `CLOSED_SHELL` / `MANIFOLD_SOLID_BREP`).
#[derive(Clone, PartialEq, Debug)]
pub struct CadSolid {
    /// The STEP entity id of the shell.
    pub id: u64,
    /// The solid's faces.
    pub faces: Vec<CadFace>,
}

/// **Neutral semantic PMI** (M15.5 / ADR-075) — one AP242 semantic feature-control-frame, as **string
/// tokens** (no foreign `Fcf` enum crosses the interchange boundary; the editor maps this ↔ its typed
/// `Characteristic`/`Standard`). It is **SEMANTIC** (a machine-readable `geometric_tolerance` entity — a
/// typed characteristic + a numeric zone + a face/datum reference), **not GRAPHICAL** (a drawn callout /
/// `annotation_occurrence` — a picture a human reads). The distinction is the whole M15.5 claim: PMI that
/// survives a STEP round-trip **still semantic**, not downgraded to a graphic.
#[derive(Clone, PartialEq, Debug)]
pub struct CadPmi {
    /// The toleranced feature — a [`CadFace`] `#id` **in this scene** (the SHAPE_ASPECT-referenced face).
    pub face_id: u64,
    /// The GD&T characteristic as a canonical token (`"position"`/`"flatness"`/… — the editor's
    /// `Characteristic::canonical()`), derived from the AP242 `geometric_tolerance` subtype entity name.
    pub characteristic: String,
    /// The tolerance-zone magnitude in millimetres (from the `LENGTH_MEASURE_WITH_UNIT`).
    pub value_mm: f64,
    /// The datum feature — a [`CadFace`] `#id` — for orientation/location tolerances; `None` for form.
    pub datum_face_id: Option<u64>,
    /// The authoring standard token (`"ASME_Y14.5"`/`"ISO_GPS"`), from the tolerance `description`.
    pub standard: String,
    /// **True** = parsed from a machine-readable `geometric_tolerance` chain (semantic); **false** = a
    /// graphical-only callout was found (a downgrade — measured, never silently treated as semantic). Our
    /// own writer only ever emits semantic entities, so a round-trip through this crate stays `true`.
    pub semantic: bool,
}

/// The neutral CAD import — our types only, no foreign STEP-lib leak (invariant 5). The editor maps this to
/// **referenceable registry entities** (faces/edges) + a tessellated `MeshAsset`, as one undoable commit.
#[derive(Clone, PartialEq, Debug)]
pub struct CadScene {
    /// A display name (from the STEP `FILE_NAME`, or the schema).
    pub name: String,
    /// The format tag (e.g. `"STEP-AP242"`).
    pub format: String,
    /// The declared units (STEP is millimetres by convention unless a `LENGTH_UNIT` says otherwise).
    pub units: Units,
    /// The solids.
    pub solids: Vec<CadSolid>,
    /// The **semantic PMI** attached to referenceable faces (M15.5) — round-tripped through STEP AP242 as
    /// machine-readable `geometric_tolerance` entities, never a graphical downgrade.
    pub pmi: Vec<CadPmi>,
    /// Every unsupported/approximated feature, explained (curved faces → the OCCT seam), never a silent drop.
    pub notes: Vec<UnsupportedNote>,
}

impl CadScene {
    /// Total referenceable face count across all solids.
    #[must_use]
    pub fn face_count(&self) -> usize {
        self.solids.iter().map(|s| s.faces.len()).sum()
    }

    /// Total referenceable edge count across all solids.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.solids
            .iter()
            .flat_map(|s| &s.faces)
            .map(|f| f.edges.len())
            .sum()
    }

    /// Tessellate the **planar** faces into a single welded [`TriMesh`] for wgpu. Vertices are welded by
    /// exact coordinate (shared corners are shared) so a closed solid tessellates **watertight**; each
    /// triangle is oriented outward via the convex-solid centroid (correct for the spike; the general
    /// non-convex case uses the parsed surface normal + `same_sense` — a named refinement). Curved faces
    /// are skipped here (their tessellation is the OCCT seam).
    #[must_use]
    pub fn tessellate(&self) -> TriMesh {
        self.tessellate_with(crate::analytic::DEFLECTION)
    }

    /// [`Self::tessellate`] at an explicit analytic deflection — the assembly bake passes
    /// [`crate::analytic::PREVIEW_DEFLECTION`] (quality by context: a 13k-occurrence factory cell at
    /// preview grade stays inside the "on screen in seconds" budget; a single part gets viewer grade).
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // polygon vertex counts are tiny
    pub fn tessellate_with(&self, deflection: f64) -> TriMesh {
        let mut weld: BTreeMap<[u64; 3], u32> = BTreeMap::new();
        let mut positions: Vec<[f64; 3]> = Vec::new();
        let mut triangles: Vec<[u32; 3]> = Vec::new();

        // Solid centroid (for outward orientation of a convex solid).
        let mut sc = [0.0f64; 3];
        let mut nc = 0.0f64;
        for solid in &self.solids {
            for face in &solid.faces {
                for v in &face.outer {
                    for k in 0..3 {
                        sc[k] += v[k];
                    }
                    nc += 1.0;
                }
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
            // The welded vertex count is bounded by the CARTESIAN_POINT entity count (≤ MAX_ENTITIES < u32::MAX
            // under the import caps), so the cast never truncates — but use a saturating fallback rather than a
            // panic so an adversarial input is NEVER a crash (the M10.2 never-panic gate, defence-in-depth).
            let i = u32::try_from(positions.len()).unwrap_or(u32::MAX);
            positions.push(p);
            weld.insert(key, i);
            i
        };

        for solid in &self.solids {
            for face in &solid.faces {
                // M15.8 (ADR-078) — an ANALYTIC curved face (cylinder/cone/sphere/torus) tessellates
                // closed-form, smooth + adaptive at the fixed absolute deflection. Beyond-subset faces
                // (complex trims / off-surface bounds) fall through silently HERE only because
                // `interpret_face` / the tessellation caller already carries the explained seam note —
                // the never-silent record lives in `notes`, not in this hot loop.
                if let Some(surface) = &face.surface {
                    if let Ok(patch) = crate::analytic::tessellate_analytic_with(
                        face.id,
                        surface,
                        &face.outer,
                        face.same_sense,
                        deflection,
                    ) {
                        // Weld the patch's grid through the shared exact-coordinate intern so seams shared
                        // between analytic faces (e.g. two half-cylinders) stitch.
                        let remap: Vec<u32> = patch
                            .positions
                            .iter()
                            .map(|&p| vid(p, &mut positions))
                            .collect();
                        for t in &patch.triangles {
                            triangles.push([
                                remap[t[0] as usize],
                                remap[t[1] as usize],
                                remap[t[2] as usize],
                            ]);
                        }
                        continue;
                    }
                }
                if face.kind != FaceKind::Planar || face.outer.len() < 3 {
                    continue;
                }
                // Face centroid (for the outward test).
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

                // Fan-triangulate the polygon around vertex 0, oriented outward.
                let i0 = vid(face.outer[0], &mut positions);
                for w in 1..face.outer.len() - 1 {
                    let ia = vid(face.outer[w], &mut positions);
                    let ib = vid(face.outer[w + 1], &mut positions);
                    push_outward(&positions, &mut triangles, [i0, ia, ib], out_dir);
                }
            }
        }
        TriMesh::new(positions, triangles)
    }
}

#[allow(clippy::many_single_char_names)] // a/b/c/n are the standard triangle/normal names
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
        return; // degenerate sliver
    }
    let dot = n[0] * out_dir[0] + n[1] * out_dir[1] + n[2] * out_dir[2];
    if dot >= 0.0 {
        triangles.push(tri);
    } else {
        triangles.push([tri[0], tri[2], tri[1]]);
    }
}

/// A STEP import/export that couldn't be honored — surfaced, never hidden (the explain discipline).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepError {
    /// The file exceeds the [`MAX_STEP_BYTES`] size cap.
    TooLarge {
        /// Actual byte length.
        bytes: usize,
        /// The cap.
        limit: usize,
    },
    /// More than [`MAX_ENTITIES`] instances (the decode-bomb guard).
    TooManyEntities {
        /// Actual instance count.
        count: usize,
        /// The cap.
        limit: usize,
    },
    /// The Part-21 structure is malformed — carries the reason (not a panic).
    Malformed(String),
    /// A `#ref` points at an entity that doesn't exist.
    DanglingRef(u64),
    /// Parsed, but no importable solid/face was found.
    Empty(String),
}

impl std::fmt::Display for StepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { bytes, limit } => {
                write!(f, "STEP file too large: {bytes} bytes > {limit} cap")
            }
            Self::TooManyEntities { count, limit } => {
                write!(f, "STEP file has too many entities: {count} > {limit} cap")
            }
            Self::Malformed(why) => write!(f, "malformed STEP: {why}"),
            Self::DanglingRef(id) => write!(f, "STEP reference #{id} points at nothing"),
            Self::Empty(why) => write!(f, "nothing to import from the STEP file: {why}"),
        }
    }
}

impl std::error::Error for StepError {}

/// The project-owned CAD interchange seam — the STEP boundary, mirroring the M8.5 [`crate::Interchange`]
/// pattern. No foreign STEP-lib type appears in any signature (invariant 5); an OCCT-backed impl (future)
/// stays behind this same trait.
pub trait CadInterchange {
    /// The format name (provenance / notes).
    fn format(&self) -> &'static str;
    /// Parse `source` bytes into a neutral [`CadScene`] (bounds-checked; malformed → explained).
    fn import(&self, source: &[u8]) -> Result<CadScene, StepError>;
    /// Re-export a [`CadScene`] to ISO-10303-21 text (faceted; geometry preserved, NURBS not — the seam).
    fn export(&self, scene: &CadScene) -> Result<String, StepError>;
}

/// The pure-Rust STEP Part-21 interchange (planar B-rep + faceted). The kernel-free exchange leg.
#[derive(Clone, Copy, Debug, Default)]
pub struct StepInterchange;

impl CadInterchange for StepInterchange {
    fn format(&self) -> &'static str {
        "STEP-AP242"
    }

    fn import(&self, source: &[u8]) -> Result<CadScene, StepError> {
        if source.len() > MAX_STEP_BYTES {
            return Err(StepError::TooLarge {
                bytes: source.len(),
                limit: MAX_STEP_BYTES,
            });
        }
        let text = std::str::from_utf8(source)
            .map_err(|_| StepError::Malformed("not valid UTF-8".into()))?;
        parse_and_interpret(text)
    }

    fn export(&self, scene: &CadScene) -> Result<String, StepError> {
        export_faceted(scene)
    }
}

// ============================================================================================
// The Part-21 parser (pure Rust, bounds-checked — never panics on bad input)
// ============================================================================================

/// A parsed Part-21 value.
#[derive(Clone, Debug, PartialEq)]
enum Value {
    Ref(u64),
    Real(f64),
    Int(i64),
    Str(String),
    Enum(String),
    List(Vec<Value>),
    /// A typed record like `LENGTH_MEASURE(5.)` — kept as (name, inner list) but rarely needed here.
    Typed(String, Vec<Value>),
    Null, // $
    Star, // *
}

impl Value {
    fn as_ref_id(&self) -> Option<u64> {
        match self {
            Value::Ref(id) => Some(*id),
            _ => None,
        }
    }
    fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(v) => Some(v),
            _ => None,
        }
    }
    #[allow(clippy::cast_precision_loss)] // STEP integers used as coordinates are small
    fn as_real(&self) -> Option<f64> {
        match self {
            Value::Real(r) => Some(*r),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Entity {
    name: String,
    args: Vec<Value>,
}

/// Strip Part-21 `/* … */` comments (outside strings).
fn strip_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_str = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            out.push(c as char);
            if c == b'\'' {
                in_str = false;
            }
            i += 1;
        } else if c == b'\'' {
            in_str = true;
            out.push(c as char);
            i += 1;
        } else if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(c as char);
            i += 1;
        }
    }
    out
}

/// Split the DATA section into `#id = NAME(...)` statements on top-level `;` (not inside strings/parens).
fn split_statements(data: &str) -> Vec<String> {
    let bytes = data.as_bytes();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            cur.push(c as char);
            if c == b'\'' {
                in_str = false;
            }
        } else {
            match c {
                b'\'' => {
                    in_str = true;
                    cur.push('\'');
                }
                b'(' => {
                    depth += 1;
                    cur.push('(');
                }
                b')' => {
                    depth -= 1;
                    cur.push(')');
                }
                b';' if depth == 0 => {
                    let s = cur.trim().to_string();
                    if !s.is_empty() {
                        out.push(s);
                    }
                    cur.clear();
                }
                _ => cur.push(c as char),
            }
        }
        i += 1;
    }
    out
}

/// Parse one `#id = NAME(args)` statement.
fn parse_statement(stmt: &str) -> Result<(u64, Entity), StepError> {
    let rest = stmt.strip_prefix('#').ok_or_else(|| {
        StepError::Malformed(format!("statement does not start with '#': {stmt:.40}"))
    })?;
    let eq = rest
        .find('=')
        .ok_or_else(|| StepError::Malformed(format!("no '=' in statement #{rest:.40}")))?;
    let id: u64 = rest[..eq]
        .trim()
        .parse()
        .map_err(|_| StepError::Malformed(format!("bad entity id in #{rest:.40}")))?;
    let body = rest[eq + 1..].trim();
    // A **complex (AND-combined) instance** — `#id = (SUBTYPE_A(...) SUBTYPE_B(...) LEAF())` — the Part-21
    // form AP242 uses for a datum-referencing geometric_tolerance. It is recorded as a synthetic
    // [`COMPLEX_INSTANCE`] entity whose args are the sub-records (each a `Value::Typed`); the PMI interpreter
    // finds the geometric_tolerance leaf among them. (`parse_paren_list` already tolerates the
    // space-separated, comma-free sub-record sequence.)
    if body.starts_with('(') {
        let mut cur = Cursor::new(body);
        let list = cur.parse_paren_list()?;
        return Ok((
            id,
            Entity {
                name: COMPLEX_INSTANCE.to_string(),
                args: list,
            },
        ));
    }
    let paren = body
        .find('(')
        .ok_or_else(|| StepError::Malformed(format!("no '(' after entity name in #{id}")))?;
    let name = body[..paren].trim().to_string();
    if name.is_empty() {
        return Err(StepError::Malformed(format!("empty entity name in #{id}")));
    }
    let args_src = &body[paren..];
    let mut cur = Cursor::new(args_src);
    let args = cur.parse_paren_list()?;
    Ok((id, Entity { name, args }))
}

/// The synthetic entity name for a parsed complex (AND-combined) instance (its args are the sub-records).
const COMPLEX_INSTANCE: &str = "!COMPLEX";

/// The maximum `(...)` nesting the recursive value parser will descend before returning a `Malformed` error.
/// Real Part-21 nests only a handful deep (a complex instance's sub-records, a coordinate list); a crafted
/// deep-nesting file (`A(((((…)))))`, within [`MAX_STEP_BYTES`]) would otherwise recurse to a **stack overflow
/// (process abort)** — a bounds-check, so an adversarial input is an explained [`StepError`], never a panic.
const MAX_PAREN_DEPTH: u32 = 256;

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: u32,
}

impl<'a> Cursor<'a> {
    fn new(s: &'a str) -> Self {
        Cursor {
            bytes: s.as_bytes(),
            pos: 0,
            depth: 0,
        }
    }
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }
    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }
    /// Parse a `(...)` list at the cursor into a Vec of Values. **Depth-bounded** ([`MAX_PAREN_DEPTH`]) — a
    /// crafted deep-nesting input is an explained `Malformed` error, never a stack-overflow abort.
    fn parse_paren_list(&mut self) -> Result<Vec<Value>, StepError> {
        self.skip_ws();
        if self.peek() != Some(b'(') {
            return Err(StepError::Malformed("expected '('".into()));
        }
        self.pos += 1;
        self.depth += 1;
        if self.depth > MAX_PAREN_DEPTH {
            return Err(StepError::Malformed(format!(
                "STEP value nesting exceeds {MAX_PAREN_DEPTH} levels (deep-nesting guard)"
            )));
        }
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b')') => {
                    self.pos += 1;
                    self.depth -= 1;
                    return Ok(items);
                }
                Some(b',') => {
                    self.pos += 1;
                }
                None => return Err(StepError::Malformed("unclosed '(' in argument list".into())),
                _ => items.push(self.parse_value()?),
            }
        }
    }
    fn parse_value(&mut self) -> Result<Value, StepError> {
        self.skip_ws();
        match self.peek() {
            None => Err(StepError::Malformed("unexpected end of value".into())),
            Some(b'#') => {
                self.pos += 1;
                let start = self.pos;
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let s = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
                s.parse::<u64>()
                    .map(Value::Ref)
                    .map_err(|_| StepError::Malformed("bad #ref".into()))
            }
            Some(b'\'') => {
                self.pos += 1;
                let mut s = String::new();
                loop {
                    match self.peek() {
                        Some(b'\'') => {
                            // '' is an escaped single quote inside a string.
                            if self.bytes.get(self.pos + 1) == Some(&b'\'') {
                                s.push('\'');
                                self.pos += 2;
                            } else {
                                self.pos += 1;
                                return Ok(Value::Str(s));
                            }
                        }
                        Some(c) => {
                            s.push(c as char);
                            self.pos += 1;
                        }
                        None => return Err(StepError::Malformed("unterminated string".into())),
                    }
                }
            }
            Some(b'(') => Ok(Value::List(self.parse_paren_list()?)),
            Some(b'$') => {
                self.pos += 1;
                Ok(Value::Null)
            }
            Some(b'*') => {
                self.pos += 1;
                Ok(Value::Star)
            }
            Some(b'.') => {
                // .ENUM.
                self.pos += 1;
                let start = self.pos;
                while let Some(c) = self.peek() {
                    if c == b'.' {
                        break;
                    }
                    self.pos += 1;
                }
                let s = std::str::from_utf8(&self.bytes[start..self.pos])
                    .unwrap_or("")
                    .to_string();
                if self.peek() == Some(b'.') {
                    self.pos += 1;
                    Ok(Value::Enum(s))
                } else {
                    Err(StepError::Malformed("unterminated .enum.".into()))
                }
            }
            Some(c) if c == b'-' || c == b'+' || c.is_ascii_digit() => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() => {
                // A bare keyword or a typed record NAME(...).
                let start = self.pos;
                while let Some(c) = self.peek() {
                    if c.is_ascii_alphanumeric() || c == b'_' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let name = std::str::from_utf8(&self.bytes[start..self.pos])
                    .unwrap_or("")
                    .to_string();
                self.skip_ws();
                if self.peek() == Some(b'(') {
                    let inner = self.parse_paren_list()?;
                    Ok(Value::Typed(name, inner))
                } else {
                    Ok(Value::Enum(name))
                }
            }
            Some(c) => Err(StepError::Malformed(format!(
                "unexpected char '{}' in value",
                c as char
            ))),
        }
    }
    fn parse_number(&mut self) -> Result<Value, StepError> {
        let start = self.pos;
        let mut is_real = false;
        if matches!(self.peek(), Some(b'-' | b'+')) {
            self.pos += 1;
        }
        while let Some(c) = self.peek() {
            match c {
                b'0'..=b'9' => self.pos += 1,
                b'.' => {
                    is_real = true;
                    self.pos += 1;
                }
                b'e' | b'E' => {
                    is_real = true;
                    self.pos += 1;
                    if matches!(self.peek(), Some(b'-' | b'+')) {
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
        let s = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
        if is_real {
            s.parse::<f64>()
                .map(Value::Real)
                .map_err(|_| StepError::Malformed(format!("bad real '{s}'")))
        } else {
            s.parse::<i64>()
                .map(Value::Int)
                .map_err(|_| StepError::Malformed(format!("bad integer '{s}'")))
        }
    }
}

/// Tokenize the whole Part-21 file text into the entity graph (`#id → Entity`). Reused by both the planar
/// B-rep interpreter and the M15.7 tessellated-assembly reader. Bounds-checked (the decode-bomb guard);
/// malformed → an explained [`StepError`], never a panic.
pub(crate) fn parse_entities(text: &str) -> Result<BTreeMap<u64, Entity>, StepError> {
    if !text.contains("ISO-10303-21") || !text.contains("END-ISO-10303-21") {
        return Err(StepError::Malformed(
            "missing ISO-10303-21 / END-ISO-10303-21 wrapper".into(),
        ));
    }
    let clean = strip_comments(text);
    let data_start = clean
        .find("DATA;")
        .ok_or_else(|| StepError::Malformed("no DATA; section".into()))?
        + "DATA;".len();
    // The DATA section ends at its ENDSEC; (the last ENDSEC before END-ISO).
    let data_end = clean[data_start..]
        .find("ENDSEC")
        .ok_or_else(|| StepError::Malformed("DATA section not closed with ENDSEC".into()))?
        + data_start;
    let data = &clean[data_start..data_end];

    let statements = split_statements(data);
    if statements.len() > MAX_ENTITIES {
        return Err(StepError::TooManyEntities {
            count: statements.len(),
            limit: MAX_ENTITIES,
        });
    }

    let mut entities: BTreeMap<u64, Entity> = BTreeMap::new();
    for stmt in &statements {
        // Skip a leading schema/complex line that isn't `#id = ...` gracefully.
        if !stmt.starts_with('#') {
            continue;
        }
        let (id, ent) = parse_statement(stmt)?;
        entities.insert(id, ent);
    }
    if entities.is_empty() {
        return Err(StepError::Empty("no entity instances in DATA".into()));
    }
    Ok(entities)
}

/// Parse the whole file text and interpret the planar B-rep + faceted subset into a [`CadScene`].
fn parse_and_interpret(text: &str) -> Result<CadScene, StepError> {
    interpret(&parse_entities(text)?)
}

/// Look up an entity, or a dangling-ref error.
fn ent(entities: &BTreeMap<u64, Entity>, id: u64) -> Result<&Entity, StepError> {
    entities.get(&id).ok_or(StepError::DanglingRef(id))
}

/// Exact point equality by bit pattern (dedup of a repeated loop vertex — never a fuzzy float compare).
fn pt_eq(a: &[f64; 3], b: &[f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

/// A CARTESIAN_POINT → [f64;3].
fn point_of(entities: &BTreeMap<u64, Entity>, id: u64) -> Result<[f64; 3], StepError> {
    let e = ent(entities, id)?;
    if e.name != "CARTESIAN_POINT" {
        return Err(StepError::Malformed(format!(
            "#{id} is {}, expected CARTESIAN_POINT",
            e.name
        )));
    }
    let coords =
        e.args.get(1).and_then(Value::as_list).ok_or_else(|| {
            StepError::Malformed(format!("#{id} CARTESIAN_POINT has no coord list"))
        })?;
    let mut p = [0.0f64; 3];
    for (k, slot) in p.iter_mut().enumerate() {
        *slot = coords
            .get(k)
            .and_then(Value::as_real)
            .ok_or_else(|| StepError::Malformed(format!("#{id} coord {k} not a real")))?;
    }
    Ok(p)
}

/// A VERTEX_POINT → its CARTESIAN_POINT coords.
fn vertex_point(entities: &BTreeMap<u64, Entity>, id: u64) -> Result<[f64; 3], StepError> {
    let e = ent(entities, id)?;
    if e.name != "VERTEX_POINT" {
        // Some files reference the CARTESIAN_POINT directly.
        if e.name == "CARTESIAN_POINT" {
            return point_of(entities, id);
        }
        return Err(StepError::Malformed(format!(
            "#{id} is {}, expected VERTEX_POINT",
            e.name
        )));
    }
    let pt = e
        .args
        .get(1)
        .and_then(Value::as_ref_id)
        .ok_or_else(|| StepError::Malformed(format!("#{id} VERTEX_POINT has no point ref")))?;
    point_of(entities, pt)
}

/// Build the CadScene from the entity graph — the planar B-rep + faceted interpreter.
pub(crate) fn interpret(entities: &BTreeMap<u64, Entity>) -> Result<CadScene, StepError> {
    let mut notes: Vec<UnsupportedNote> = Vec::new();

    // Find the shells: every CLOSED_SHELL / OPEN_SHELL (directly, or referenced by a MANIFOLD_SOLID_BREP /
    // FACETED_BREP / *_BREP). Collect shell ids so a solid maps 1:1 to a shell.
    let mut shell_ids: Vec<u64> = Vec::new();
    for (id, e) in entities {
        if e.name == "CLOSED_SHELL" || e.name == "OPEN_SHELL" {
            shell_ids.push(*id);
        }
    }
    if shell_ids.is_empty() {
        return Err(StepError::Empty(
            "no CLOSED_SHELL / OPEN_SHELL — not a B-rep this planar importer handles (a curved-only or \
             wireframe file rides the OCCT seam)"
                .into(),
        ));
    }

    let mut solids = Vec::new();
    for shell_id in shell_ids {
        let shell = ent(entities, shell_id)?;
        let face_refs =
            shell.args.get(1).and_then(Value::as_list).ok_or_else(|| {
                StepError::Malformed(format!("shell #{shell_id} has no face list"))
            })?;
        let mut faces = Vec::new();
        for fr in face_refs {
            let Some(fid) = fr.as_ref_id() else { continue };
            faces.push(interpret_face(entities, fid, &mut notes)?);
        }
        if !faces.is_empty() {
            solids.push(CadSolid {
                id: shell_id,
                faces,
            });
        }
    }
    if solids.is_empty() {
        return Err(StepError::Empty(
            "shells present but no faces resolved".into(),
        ));
    }

    // Parse the semantic PMI (AP242 geometric_tolerance entities) attached to the resolved faces (M15.5).
    let face_ids: std::collections::BTreeSet<u64> =
        solids.iter().flat_map(|s| &s.faces).map(|f| f.id).collect();
    let pmi = parse_pmi(entities, &face_ids, &mut notes);

    let name = file_name(entities).unwrap_or_else(|| "STEP part".to_string());
    Ok(CadScene {
        name,
        format: "STEP-AP242".into(),
        // STEP length unit is millimetres by convention; expose as metres-per-unit for the M8.3 check.
        units: Units {
            meters_per_unit: 0.001,
            kilograms_per_unit: 1.0,
        },
        solids,
        pmi,
        notes,
    })
}

/// Interpret an ADVANCED_FACE / FACE_SURFACE / FACE into a referenceable CadFace + its boundary polygon.
fn interpret_face(
    entities: &BTreeMap<u64, Entity>,
    fid: u64,
    notes: &mut Vec<UnsupportedNote>,
) -> Result<CadFace, StepError> {
    let f = ent(entities, fid)?;
    // ADVANCED_FACE('', (#bound...), #surface, same_sense) — bounds are arg 1, surface arg 2.
    let bounds = f
        .args
        .get(1)
        .and_then(Value::as_list)
        .ok_or_else(|| StepError::Malformed(format!("face #{fid} has no bound list")))?;
    let surface_id = f.args.get(2).and_then(Value::as_ref_id);
    let same_sense = !matches!(f.args.get(3), Some(Value::Enum(e)) if e == "F");

    // Classify the surface: PLANE (or a faceted FACE with no surface entity) → tessellated here; an
    // ANALYTIC curved surface (cylinder/cone/sphere/torus — M15.8/ADR-078) → recognized + tessellated
    // closed-form; NURBS/freeform → referenced but the licensed-kernel/OCCT seam (never hand-rolled).
    let mut surface = None;
    let kind = match surface_id.and_then(|sid| entities.get(&sid)) {
        Some(s) if s.name == "PLANE" => FaceKind::Planar,
        Some(s) => {
            surface = analytic_surface_of(entities, s);
            if surface.is_none() {
                notes.push(UnsupportedNote {
                    feature: format!("{} on face #{fid}", s.name),
                    detail: "curved/freeform surface — referenced (M15.3 PMI can attach) but NOT \
                             tessellated here; exact tessellation is the OpenCascade native/server seam \
                             (ADR-070)"
                        .into(),
                });
            }
            FaceKind::Curved
        }
        // A faceted FACE (FACETED_BREP) carries no surface entity — it is a planar polygon facet.
        None => FaceKind::Planar,
    };

    // The outer boundary: first FACE_OUTER_BOUND (fallback: first FACE_BOUND) → loop → ordered vertices.
    let mut outer: Vec<[f64; 3]> = Vec::new();
    let mut edges: Vec<CadEdge> = Vec::new();
    let mut got_outer = false;
    for br in bounds {
        let Some(bid) = br.as_ref_id() else { continue };
        let b = ent(entities, bid)?;
        let is_outer = b.name == "FACE_OUTER_BOUND";
        if got_outer && is_outer {
            continue;
        }
        let loop_id = b
            .args
            .get(1)
            .and_then(Value::as_ref_id)
            .ok_or_else(|| StepError::Malformed(format!("bound #{bid} has no loop")))?;
        let (poly, es) = interpret_loop(entities, loop_id)?;
        if is_outer || !got_outer {
            outer = poly;
            edges = es;
            got_outer = is_outer || got_outer;
            if is_outer {
                got_outer = true;
            }
        }
    }

    // The declared-subset gate runs at parse time (where the notes live): a recognized analytic face whose
    // boundary can't be tessellated (off-surface bounds / a non-rectangular trim / a degenerate patch)
    // DOWNGRADES to the explained seam note — every curved face either renders smooth or is accounted for,
    // never silent.
    if let Some(s) = &surface {
        if let Err(note) = crate::analytic::plan_analytic(fid, s, &outer) {
            notes.push(note);
            surface = None;
        }
    }

    Ok(CadFace {
        id: fid,
        kind,
        outer,
        edges,
        surface,
        same_sense,
    })
}

/// Recognize the four ANALYTIC surface kinds (M15.8/ADR-078) into their closed forms. `None` for anything
/// else (NURBS/freeform — the kernel seam). Arg shapes:
/// `CYLINDRICAL_SURFACE('', #placement, radius)` · `CONICAL_SURFACE('', #placement, radius, semi_angle)` ·
/// `SPHERICAL_SURFACE('', #placement, radius)` · `TOROIDAL_SURFACE('', #placement, major, minor)`.
fn analytic_surface_of(
    entities: &BTreeMap<u64, Entity>,
    s: &Entity,
) -> Option<crate::analytic::AnalyticSurface> {
    use crate::analytic::AnalyticSurface as A;
    let frame = s
        .args
        .get(1)
        .and_then(Value::as_ref_id)
        .map(|p| axis_placement_matrix(entities, p))?;
    let real = |i: usize| s.args.get(i).and_then(Value::as_real);
    let positive = |x: f64| (x.is_finite() && x > 0.0).then_some(x);
    match s.name.as_str() {
        "CYLINDRICAL_SURFACE" => Some(A::Cylinder {
            frame,
            radius: positive(real(2)?)?,
        }),
        "CONICAL_SURFACE" => Some(A::Cone {
            frame,
            radius: positive(real(2)?)?,
            // The semi-angle is in the file's plane_angle unit (radians by convention in AP242 exports we
            // read); a degenerate/reflex angle is beyond the subset.
            semi_angle: real(3)
                .filter(|a| a.is_finite() && a.abs() < std::f64::consts::FRAC_PI_2)?,
        }),
        "SPHERICAL_SURFACE" => Some(A::Sphere {
            frame,
            radius: positive(real(2)?)?,
        }),
        "TOROIDAL_SURFACE" => {
            let major = positive(real(2)?)?;
            let minor = positive(real(3)?)?;
            (minor < major).then_some(A::Torus {
                frame,
                major,
                minor,
            })
        }
        _ => None,
    }
}

/// Interpret an EDGE_LOOP (advanced b-rep) or POLY_LOOP (faceted) into an ordered vertex polygon + edges.
fn interpret_loop(
    entities: &BTreeMap<u64, Entity>,
    loop_id: u64,
) -> Result<(Vec<[f64; 3]>, Vec<CadEdge>), StepError> {
    let l = ent(entities, loop_id)?;
    match l.name.as_str() {
        "POLY_LOOP" => {
            // POLY_LOOP('', (#cartesian_point...)) — a direct polygon (faceted b-rep).
            let pts = l.args.get(1).and_then(Value::as_list).ok_or_else(|| {
                StepError::Malformed(format!("POLY_LOOP #{loop_id} has no points"))
            })?;
            let mut poly = Vec::new();
            for pr in pts {
                if let Some(pid) = pr.as_ref_id() {
                    poly.push(point_of(entities, pid)?);
                }
            }
            let mut edges = Vec::new();
            for w in 0..poly.len() {
                edges.push(CadEdge {
                    id: loop_id, // faceted loops have no per-edge id; key on the loop
                    ends: [poly[w], poly[(w + 1) % poly.len()]],
                });
            }
            Ok((poly, edges))
        }
        "EDGE_LOOP" => {
            // EDGE_LOOP('', (#oriented_edge...)) — traverse the oriented edges to ordered vertices.
            let oes = l.args.get(1).and_then(Value::as_list).ok_or_else(|| {
                StepError::Malformed(format!("EDGE_LOOP #{loop_id} has no edges"))
            })?;
            let mut poly: Vec<[f64; 3]> = Vec::new();
            let mut edges: Vec<CadEdge> = Vec::new();
            for oer in oes {
                let Some(oeid) = oer.as_ref_id() else {
                    continue;
                };
                let oe = ent(entities, oeid)?;
                // ORIENTED_EDGE('', *edge_start, *edge_end, #edge_curve, orientation) — args 3 and 4.
                let ec_id = oe.args.get(3).and_then(Value::as_ref_id).ok_or_else(|| {
                    StepError::Malformed(format!("ORIENTED_EDGE #{oeid} no edge"))
                })?;
                let orientation = matches!(oe.args.get(4), Some(Value::Enum(e)) if e == "T");
                let ec = ent(entities, ec_id)?;
                // EDGE_CURVE('', #v1, #v2, #geom, same_sense)
                let v1 =
                    ec.args.get(1).and_then(Value::as_ref_id).ok_or_else(|| {
                        StepError::Malformed(format!("EDGE_CURVE #{ec_id} no v1"))
                    })?;
                let v2 =
                    ec.args.get(2).and_then(Value::as_ref_id).ok_or_else(|| {
                        StepError::Malformed(format!("EDGE_CURVE #{ec_id} no v2"))
                    })?;
                let (pa, pb) = (vertex_point(entities, v1)?, vertex_point(entities, v2)?);
                let (start, _end) = if orientation { (pa, pb) } else { (pb, pa) };
                // Append the start vertex of this oriented edge (dedup a repeated last==first).
                if poly.last().is_none_or(|last| !pt_eq(last, &start)) {
                    poly.push(start);
                }
                edges.push(CadEdge {
                    id: ec_id,
                    ends: [pa, pb],
                });
            }
            // Drop a trailing vertex equal to the first (closed loops repeat).
            if poly.len() > 1
                && matches!((poly.first(), poly.last()), (Some(a), Some(b)) if pt_eq(a, b))
            {
                poly.pop();
            }
            Ok((poly, edges))
        }
        other => Err(StepError::Malformed(format!(
            "loop #{loop_id} is {other}, expected EDGE_LOOP or POLY_LOOP"
        ))),
    }
}

/// Best-effort file name from FILE_NAME's first string arg.
pub(crate) fn file_name(entities: &BTreeMap<u64, Entity>) -> Option<String> {
    for e in entities.values() {
        if e.name == "PRODUCT" {
            if let Some(Value::Str(s)) = e.args.first() {
                if !s.is_empty() {
                    return Some(s.clone());
                }
            }
        }
    }
    None
}

// ============================================================================================
// Tessellated-assembly AP242 reader (M15.7 / ADR-077) — the embedded-tessellation + assembly-placement leg
// ============================================================================================
//
// A large curved commercial-CAD STEP (the "wild-CAD" case the planar B-rep subset can't cover — cylinders,
// NURBS, thousands of parts) still carries an OPEN, readable **visualization tessellation**
// (TESSELLATED_SOLID → TRIANGULATED_FACE → COORDINATES_LIST) plus the assembly-placement chain
// (NEXT_ASSEMBLY_USAGE_OCCURRENCE → ITEM_DEFINED_TRANSFORMATION). This reader shows that cache — real
// triangulated geometry, correctly placed — with NO kernel (the tessellation is standard STEP; exact curved
// B-rep is still the OCCT seam). This is the "like a texture" path for a commercial STEP.

/// One placed tessellated part from a STEP assembly.
pub(crate) struct TessPart {
    pub name: String,
    /// The product-definition `#id` (the dedup / re-import key).
    pub reference: String,
    /// Column-major world transform (composed down the assembly tree).
    pub transform: [f64; 16],
    /// The welded triangulated mesh (from the embedded tessellation).
    pub mesh: TriMesh,
    /// The authored display colour (linear RGB) resolved from the STEP `STYLED_ITEM` chain, if any — so the
    /// part renders in its real colour instead of a uniform default.
    pub color: Option<[f32; 3]>,
}

/// Read a STEP file's embedded tessellation into placed [`TessPart`]s. Returns an empty Vec if the file
/// carries no tessellation (the caller falls back to the planar B-rep interpreter).
///
/// **The real AP242 tessellated-assembly structure** (what commercial CAD — CATIA/NX — actually exports, and
/// what this file uses): the geometry + placement live entirely in a nested tessellation graph, NOT in the
/// NAUO/PRODUCT_DEFINITION assembly. A node is a complex entity
/// `(GEOMETRIC_REPRESENTATION_ITEM() REPOSITIONED_TESSELLATED_ITEM(#axis) REPRESENTATION_ITEM('')
/// TESSELLATED_GEOMETRIC_SET((#children)) TESSELLATED_ITEM())` — a *set* of child items, repositioned as a
/// whole by `#axis` (an `AXIS2_PLACEMENT_3D`). Children are more such nodes (nested sub-assemblies) or leaf
/// `TESSELLATED_SOLID`/`TESSELLATED_SHELL`s. Each leaf becomes one placed part; its world transform is the
/// composition of every ancestor node's reposition axis. This captures the curved surfaces (cylinders / cones
/// / B-splines) the planar B-rep reader can only proxy — the tessellation triangulates every surface.
pub(crate) fn parse_tessellated_assembly(entities: &BTreeMap<u64, Entity>) -> Vec<TessPart> {
    // A quick check: any tessellation at all?
    if !entities.values().any(is_tess_node) {
        return Vec::new();
    }

    // Every id that is a CHILD of some tessellation container — so roots are the nodes at the top of the
    // tessellation forest (the items a SHAPE_REPRESENTATION would carry), which we place at identity.
    let mut is_child: BTreeSet<u64> = BTreeSet::new();
    for e in entities.values() {
        let (_, children) = tess_container(entities, e);
        for c in children {
            is_child.insert(c);
        }
    }
    let mut roots: Vec<u64> = entities
        .iter()
        .filter(|(id, e)| is_tess_node(e) && !is_child.contains(id))
        .map(|(id, _)| *id)
        .collect();
    roots.sort_unstable();

    // Per-geometry authored colours (from the STEP `STYLED_ITEM` chain) so each part renders in its real
    // colour, keyed by the styled item id (the tessellated solid / geometric-set node).
    let colors = styled_colors(entities);

    let mut out: Vec<TessPart> = Vec::new();
    let mut on_path: BTreeSet<u64> = BTreeSet::new();
    for root in roots {
        on_path.clear();
        walk_tess(
            entities,
            root,
            crate::cad_import::IDENTITY_4X4,
            0,
            &colors,
            &mut on_path,
            &mut out,
        );
    }
    out
}

/// Map a styled geometry item id → its authored display colour (linear RGB), from the STEP presentation model:
/// `STYLED_ITEM(name, (styles), item)` where a bounded search from each style ref reaches a `COLOUR_RGB` (or a
/// `DRAUGHTING_PRE_DEFINED_COLOUR` name) through the `PRESENTATION_STYLE_ASSIGNMENT → SURFACE_STYLE_USAGE →
/// SURFACE_SIDE_STYLE → SURFACE_STYLE_FILL_AREA → FILL_AREA_STYLE → FILL_AREA_STYLE_COLOUR` (or
/// `SURFACE_STYLE_RENDERING`) chain. Structure-agnostic: rather than hard-code every leg, it walks refs to the
/// first colour (bounded depth). The colour is keyed on the `STYLED_ITEM.item` (commonly the `TESSELLATED_SOLID`).
fn styled_colors(entities: &BTreeMap<u64, Entity>) -> BTreeMap<u64, [f32; 3]> {
    let mut out: BTreeMap<u64, [f32; 3]> = BTreeMap::new();
    for e in entities.values() {
        if e.name != "STYLED_ITEM" {
            continue;
        }
        // STYLED_ITEM(name, (styles), item) — the item is the last ref; styles are the list of style refs.
        let refs: Vec<u64> = e.args.iter().filter_map(Value::as_ref_id).collect();
        let Some(&item) = refs.last() else { continue };
        let styles = e
            .args
            .iter()
            .find_map(Value::as_list)
            .map(|l| l.iter().filter_map(Value::as_ref_id).collect::<Vec<_>>())
            .unwrap_or_default();
        for s in styles {
            if let Some(c) = resolve_colour(entities, s, 0) {
                out.entry(item).or_insert(c);
                break;
            }
        }
    }
    out
}

/// Follow refs from a style entity to the first `COLOUR_RGB` (linear RGB) or a named pre-defined colour,
/// bounded in depth (the presentation graph is shallow). Structure-agnostic so it tolerates the AP242 style
/// variants (`SURFACE_STYLE_RENDERING` vs `FILL_AREA_STYLE`).
fn resolve_colour(entities: &BTreeMap<u64, Entity>, id: u64, depth: u32) -> Option<[f32; 3]> {
    if depth > 8 {
        return None;
    }
    let e = entities.get(&id)?;
    if e.name == "COLOUR_RGB" {
        // COLOUR_RGB(name, r, g, b) — the three reals are already 0..1 linear.
        let vals: Vec<f64> = e.args.iter().filter_map(Value::as_real).collect();
        if let [r, g, b] = vals[..] {
            #[allow(clippy::cast_possible_truncation)]
            return Some([r as f32, g as f32, b as f32]);
        }
        return None;
    }
    if e.name == "DRAUGHTING_PRE_DEFINED_COLOUR" || e.name == "PRE_DEFINED_COLOUR" {
        if let Some(Value::Str(name)) = e.args.first() {
            return predefined_colour(name);
        }
    }
    // Otherwise recurse into every ref reachable in this style entity's args — INCLUDING refs nested inside
    // lists (`PRESENTATION_STYLE_ASSIGNMENT((#style))`) and complex sub-records — taking the first colour.
    let mut refs = Vec::new();
    for a in &e.args {
        collect_refs(a, &mut refs);
    }
    for r in refs {
        if let Some(c) = resolve_colour(entities, r, depth + 1) {
            return Some(c);
        }
    }
    None
}

/// Collect every `#ref` reachable in a value, descending into lists + complex sub-records (the presentation
/// graph nests its refs inside lists, e.g. `PRESENTATION_STYLE_ASSIGNMENT((#surface_style))`).
fn collect_refs(v: &Value, out: &mut Vec<u64>) {
    match v {
        Value::Ref(id) => out.push(*id),
        Value::List(items) | Value::Typed(_, items) => {
            for it in items {
                collect_refs(it, out);
            }
        }
        _ => {}
    }
}

/// The ISO-10303 pre-defined colour names → linear RGB.
fn predefined_colour(name: &str) -> Option<[f32; 3]> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "red" => [1.0, 0.0, 0.0],
        "green" => [0.0, 1.0, 0.0],
        "blue" => [0.0, 0.0, 1.0],
        "yellow" => [1.0, 1.0, 0.0],
        "magenta" => [1.0, 0.0, 1.0],
        "cyan" => [0.0, 1.0, 1.0],
        "black" => [0.0, 0.0, 0.0],
        "white" => [1.0, 1.0, 1.0],
        _ => return None,
    })
}

/// Is `e` a node in the tessellation graph (a leaf solid/shell, a set, a reposition, or a complex entity that
/// combines those)?
fn is_tess_node(e: &Entity) -> bool {
    matches!(
        e.name.as_str(),
        "TESSELLATED_SOLID"
            | "TESSELLATED_SHELL"
            | "TESSELLATED_GEOMETRIC_SET"
            | "REPOSITIONED_TESSELLATED_ITEM"
    ) || (e.name == COMPLEX_INSTANCE
        && e.args
            .iter()
            .any(|a| matches!(a, Value::Typed(n, _) if n.contains("TESSELLATED"))))
}

/// A container node's `(reposition axis, child ids)`. A leaf `TESSELLATED_SOLID`/`SHELL` (or a non-tess entity)
/// returns `(None, [])` — its geometry is read by [`mesh_of_tessellated_solid`], not recursed into.
fn tess_container(entities: &BTreeMap<u64, Entity>, e: &Entity) -> (Option<u64>, Vec<u64>) {
    if e.name == COMPLEX_INSTANCE {
        // The complex node combines REPOSITIONED_TESSELLATED_ITEM(#axis) + TESSELLATED_GEOMETRIC_SET((#kids)).
        let axis = e.args.iter().find_map(|a| match a {
            Value::Typed(n, inner) if n == "REPOSITIONED_TESSELLATED_ITEM" => {
                inner.iter().find_map(Value::as_ref_id)
            }
            _ => None,
        });
        let children = e
            .args
            .iter()
            .find_map(|a| match a {
                Value::Typed(n, inner) if n == "TESSELLATED_GEOMETRIC_SET" => Some(refs_in(inner)),
                _ => None,
            })
            .unwrap_or_default();
        (axis, children)
    } else if e.name == "TESSELLATED_GEOMETRIC_SET" {
        // Plain form: TESSELLATED_GEOMETRIC_SET(name, (#kids)).
        (None, refs_in(&e.args))
    } else if e.name == "REPOSITIONED_TESSELLATED_ITEM" {
        // Plain form: REPOSITIONED_TESSELLATED_ITEM(name, #item, #location) — the AXIS2 arg is the location.
        let refs: Vec<u64> = e.args.iter().filter_map(Value::as_ref_id).collect();
        let axis = refs.iter().copied().find(|r| {
            entities
                .get(r)
                .is_some_and(|x| x.name == "AXIS2_PLACEMENT_3D")
        });
        let children = refs.into_iter().filter(|r| Some(*r) != axis).collect();
        (axis, children)
    } else {
        (None, Vec::new())
    }
}

/// The `#ref`s inside a set of args: the first non-empty list-of-refs, else any direct `#ref` args.
fn refs_in(values: &[Value]) -> Vec<u64> {
    for v in values {
        if let Some(list) = v.as_list() {
            let refs: Vec<u64> = list.iter().filter_map(Value::as_ref_id).collect();
            if !refs.is_empty() {
                return refs;
            }
        }
    }
    values.iter().filter_map(Value::as_ref_id).collect()
}

/// Walk one tessellation node: emit a placed part for a leaf solid/shell, else compose this container's
/// reposition axis onto `world` and recurse into its children.
fn walk_tess(
    entities: &BTreeMap<u64, Entity>,
    id: u64,
    world: [f64; 16],
    depth: u32,
    colors: &BTreeMap<u64, [f32; 3]>,
    on_path: &mut BTreeSet<u64>,
    out: &mut Vec<TessPart>,
) {
    if depth > crate::cad_import::MAX_ASSEMBLY_DEPTH
        || out.len() >= 4_000_000
        || on_path.contains(&id)
    {
        return;
    }
    let Some(e) = entities.get(&id) else { return };
    if e.name == "TESSELLATED_SOLID" || e.name == "TESSELLATED_SHELL" {
        // Never-silent on the tessellation path: EVERY reachable leaf becomes a placed part. A leaf that
        // decodes to real triangles is `TessellationOnly`; one that yields NO usable mesh (unreadable faces /
        // 0 triangles) is emitted with an EMPTY mesh so `build_import` routes it to a diagnosed bounding proxy
        // at its real transform — classified + placed, never dropped without a report entry.
        let mesh = mesh_of_tessellated_solid(entities, id)
            .unwrap_or_else(|| TriMesh::new(Vec::new(), Vec::new()));
        out.push(TessPart {
            name: format!("part #{id}"),
            reference: id.to_string(),
            transform: world,
            mesh,
            color: colors.get(&id).copied(),
        });
        return;
    }
    let (axis, children) = tess_container(entities, e);
    let local = axis.map_or(crate::cad_import::IDENTITY_4X4, |a| {
        axis_placement_matrix(entities, a)
    });
    let child_world = crate::cad_import::mat4_mul(&world, &local);
    on_path.insert(id);
    for c in children {
        walk_tess(entities, c, child_world, depth + 1, colors, on_path, out);
    }
    on_path.remove(&id);
}

/// A `TESSELLATED_SOLID`/`SHELL` → its welded [`TriMesh`]. The faces of a solid typically SHARE one
/// `COORDINATES_LIST`, so we intern each coords list ONCE per solid (base offset) and rebase every face's
/// triangle indices into that shared vertex buffer — avoiding the O(faces × coords) vertex blow-up.
fn mesh_of_tessellated_solid(entities: &BTreeMap<u64, Entity>, id: u64) -> Option<TriMesh> {
    let e = entities.get(&id)?;
    // TESSELLATED_SOLID/SHELL(name, (#face_refs), …) — the faces are the first list-of-refs arg.
    let faces = refs_in(&e.args);
    if faces.is_empty() {
        return None;
    }
    let mut positions: Vec<[f64; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut base_of: BTreeMap<u64, (u32, u32)> = BTreeMap::new(); // coords id → (base offset, count)
    for fid in faces {
        let Some((coords_id, tris)) = face_triangles(entities, fid) else {
            continue;
        };
        let (base, count) = *base_of.entry(coords_id).or_insert_with(|| {
            #[allow(clippy::cast_possible_truncation)]
            let base = positions.len() as u32;
            let pts = coordinates_list(entities, coords_id).unwrap_or_default();
            #[allow(clippy::cast_possible_truncation)]
            let count = pts.len() as u32;
            positions.extend(pts);
            (base, count)
        });
        for t in tris {
            if t[0] < count && t[1] < count && t[2] < count {
                triangles.push([t[0] + base, t[1] + base, t[2] + base]);
            }
        }
    }
    if triangles.is_empty() {
        return None;
    }
    Some(TriMesh::new(positions, triangles))
}

// ============================================================================================
// The AP242 **exact B-rep assembly** reader (ADR-077 follow-up): place every product's exact
// B-rep solids at their real world transform by walking the NEXT_ASSEMBLY_USAGE_OCCURRENCE
// product tree. Commercial STEP exports (this 3DEXPERIENCE crane) carry curved/cast parts as
// tessellation (handled by `parse_tessellated_assembly`) and the STRUCTURAL steel — plates,
// beams, gussets — as exact `MANIFOLD_SOLID_BREP` in per-product shape reps at LOCAL coords,
// placed only by the PDM assembly graph. Without this walk those (the bulk of the model) are
// invisible. Planar faces are tessellated exactly (`CadScene::tessellate`); curved faces ride the
// OCCT seam (a bolt hole / fillet is missing, the plate is not).
// ============================================================================================

/// The bound on placed B-rep occurrences (the decode-bomb / runaway-instancing guard).
const MAX_BREP_PARTS: usize = 2_000_000;

/// A product's tessellated B-rep, cached by product-definition id so an instanced part is triangulated
/// once and cloned per occurrence: `(local welded mesh, authored colour, display name)`.
type BrepMesh = (TriMesh, Option<[f32; 3]>, String);

/// A single B-rep solid whose local geometry spans more than this in any axis is a **construction /
/// reference artifact** (an unbounded plane exported as a giant plate, a symmetry body), not a real part —
/// dropped so it never blows up the assembly's bounding box + camera framing. Generous: no physical member
/// of a crane / weld station is a single 120 m solid (the real ones here top out ~30 m), while the artifacts
/// are 262 m.
const MAX_PART_EXTENT_MM: f64 = 120_000.0;

/// Whether the file carries a `NEXT_ASSEMBLY_USAGE_OCCURRENCE` product-assembly graph — the signal that
/// the B-rep **assembly** union leg applies. A single-product file with no assembly graph must instead
/// take the exact planar-B-rep leg (exact fidelity + PMI + interpret()'s notes), not be hijacked into a
/// "tessellation-only" diagnosis by the mere presence of its standard PRODUCT_DEFINITION/SDR structure.
pub(crate) fn has_nauo(entities: &BTreeMap<u64, Entity>) -> bool {
    entities
        .values()
        .any(|e| e.name == "NEXT_ASSEMBLY_USAGE_OCCURRENCE")
}

/// Walk the STEP product-assembly tree and place each product's exact B-rep solids at their composed
/// world transform. Returns the placed parts (empty when the file has no NAUO product tree / no B-rep —
/// the caller still has the tessellated-assembly parts and the planar single-file fallback) plus the
/// **never-silent** notes: every curved-only product proxied, every construction-artifact solid filtered,
/// and every per-face curved-surface approximation is named, never dropped without a word.
pub(crate) fn parse_brep_assembly(
    entities: &BTreeMap<u64, Entity>,
) -> (Vec<TessPart>, Vec<UnsupportedNote>) {
    let mut notes: Vec<UnsupportedNote> = Vec::new();
    // (1) product_definition → ALL its SHAPE_REPRESENTATIONs.
    let pd_to_sr = collect_pd_shape_reps(entities);
    if pd_to_sr.is_empty() {
        return (Vec::new(), notes);
    }

    // (2) Each NAUO occurrence's placement transform.
    let nauo_xform = collect_nauo_transforms(entities);

    // (3) The assembly edges (parent product-definition → (child, nauo)), and the roots (a
    // product-definition that is never a child).
    let mut children: BTreeMap<u64, Vec<(u64, u64)>> = BTreeMap::new();
    let mut is_child: BTreeSet<u64> = BTreeSet::new();
    let mut all_pd: BTreeSet<u64> = BTreeSet::new();
    for (id, e) in entities {
        if e.name != "NEXT_ASSEMBLY_USAGE_OCCURRENCE" {
            continue;
        }
        let refs: Vec<u64> = e.args.iter().filter_map(Value::as_ref_id).collect();
        // NAUO(id, name, desc, relating_pd, related_pd, ref) — the last two refs are parent, child.
        let (Some(&parent), Some(&child)) = (refs.get(refs.len().wrapping_sub(2)), refs.last())
        else {
            continue;
        };
        children.entry(parent).or_default().push((child, *id));
        is_child.insert(child);
        all_pd.insert(parent);
        all_pd.insert(child);
    }
    let mut roots: Vec<u64> = all_pd
        .iter()
        .copied()
        .filter(|p| !is_child.contains(p))
        .collect();
    // A single product with geometry but no assembly graph is its own root.
    if roots.is_empty() {
        roots = pd_to_sr.keys().copied().collect();
    }
    roots.sort_unstable();

    // (4) Walk, composing transforms, tessellating each product's B-rep ONCE (cached — an instanced
    // bolt is tessellated once and cloned per occurrence; land_cad dedups the GPU mesh by hash).
    let colors = styled_colors(entities);
    let mut mesh_cache: BTreeMap<u64, Option<BrepMesh>> = BTreeMap::new();
    let mut out: Vec<TessPart> = Vec::new();
    let mut on_path: BTreeSet<u64> = BTreeSet::new();
    let mut reached: BTreeSet<u64> = BTreeSet::new();
    for root in roots {
        walk_brep(
            entities,
            root,
            crate::cad_import::IDENTITY_4X4,
            0,
            &pd_to_sr,
            &children,
            &nauo_xform,
            &colors,
            &mut mesh_cache,
            &mut on_path,
            &mut reached,
            &mut notes,
            &mut out,
        );
    }
    // (5) NEVER-SILENT: a product with geometry that is OUTSIDE the NAUO graph (a loose reference part, a
    // second model in the same export, a fixture alongside the assembly) is never a child of any root —
    // walk it at identity so its geometry still lands instead of silently vanishing.
    let unreached: Vec<u64> = pd_to_sr
        .keys()
        .copied()
        .filter(|pd| !reached.contains(pd))
        .collect();
    for pd in unreached {
        walk_brep(
            entities,
            pd,
            crate::cad_import::IDENTITY_4X4,
            0,
            &pd_to_sr,
            &children,
            &nauo_xform,
            &colors,
            &mut mesh_cache,
            &mut on_path,
            &mut reached,
            &mut notes,
            &mut out,
        );
    }
    (out, notes)
}

/// Each `NEXT_ASSEMBLY_USAGE_OCCURRENCE` → its placement transform. A PRODUCT_DEFINITION_SHAPE whose
/// definition is a NAUO is that occurrence's shape → find the
/// `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(rep_rel, pds)` that carries its ITEM_DEFINED_TRANSFORMATION.
fn collect_nauo_transforms(entities: &BTreeMap<u64, Entity>) -> BTreeMap<u64, [f64; 16]> {
    let mut pds_to_nauo: BTreeMap<u64, u64> = BTreeMap::new();
    for (id, e) in entities {
        if e.name == "PRODUCT_DEFINITION_SHAPE" {
            if let Some(def) = e.args.iter().filter_map(Value::as_ref_id).next_back() {
                if entities
                    .get(&def)
                    .is_some_and(|x| x.name == "NEXT_ASSEMBLY_USAGE_OCCURRENCE")
                {
                    pds_to_nauo.insert(*id, def);
                }
            }
        }
    }
    let mut nauo_xform: BTreeMap<u64, [f64; 16]> = BTreeMap::new();
    for e in entities.values() {
        if e.name != "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION" {
            continue;
        }
        let refs: Vec<u64> = e.args.iter().filter_map(Value::as_ref_id).collect();
        let (Some(&rep_rel), Some(&pds)) = (refs.first(), refs.get(1)) else {
            continue;
        };
        let Some(&nauo) = pds_to_nauo.get(&pds) else {
            continue;
        };
        if let Some(t) = occurrence_transform(entities, rep_rel) {
            nauo_xform.insert(nauo, t);
        }
    }
    nauo_xform
}

/// product_definition → ALL its SHAPE_REPRESENTATIONs (via `SHAPE_DEFINITION_REPRESENTATION(PDS(pd), sr)`).
/// A multimap: AP242 exporters routinely attach several shape reps to one PRODUCT_DEFINITION (a
/// geometry-bearing brep rep + a placement-only rep) — keeping only one would silently drop whichever rep
/// lost the insert order.
fn collect_pd_shape_reps(entities: &BTreeMap<u64, Entity>) -> BTreeMap<u64, Vec<u64>> {
    let mut pd_to_sr: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for e in entities.values() {
        if e.name != "SHAPE_DEFINITION_REPRESENTATION" {
            continue;
        }
        let refs: Vec<u64> = e.args.iter().filter_map(Value::as_ref_id).collect();
        let (Some(&pds), Some(&sr)) = (refs.first(), refs.get(1)) else {
            continue;
        };
        if let Some(pd) = pds_definition(entities, pds) {
            if entities
                .get(&pd)
                .is_some_and(|x| x.name == "PRODUCT_DEFINITION")
            {
                pd_to_sr.entry(pd).or_default().push(sr);
            }
        }
    }
    pd_to_sr
}

/// A `PRODUCT_DEFINITION_SHAPE` / `..._SHAPE_ASPECT` → the definition ref it characterises (a
/// `PRODUCT_DEFINITION` for a part shape, or a `NEXT_ASSEMBLY_USAGE_OCCURRENCE` for an occurrence shape).
fn pds_definition(entities: &BTreeMap<u64, Entity>, pds: u64) -> Option<u64> {
    let e = entities.get(&pds)?;
    // PRODUCT_DEFINITION_SHAPE(name, desc, #definition) — the definition is the last ref.
    e.args
        .iter()
        .filter_map(Value::as_ref_id)
        .next_back()
        .filter(|d| entities.contains_key(d))
}

/// The relative placement transform an occurrence's representation-relationship carries: follow the
/// `rep_rel` (a complex `REPRESENTATION_RELATIONSHIP + REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION +
/// SHAPE_REPRESENTATION_RELATIONSHIP`) to its `ITEM_DEFINED_TRANSFORMATION(item1, item2)` and compose
/// `M(item2) · M(item1)⁻¹` (item1 is the local origin frame, item2 the placement in the parent).
fn occurrence_transform(entities: &BTreeMap<u64, Entity>, rep_rel: u64) -> Option<[f64; 16]> {
    let e = entities.get(&rep_rel)?;
    // The IDT is reachable from the (complex) rep-relationship — collect its refs and take the first
    // that resolves to an ITEM_DEFINED_TRANSFORMATION.
    let mut refs = Vec::new();
    for a in &e.args {
        collect_refs(a, &mut refs);
    }
    let idt = refs.into_iter().find(|r| {
        entities
            .get(r)
            .is_some_and(|x| x.name == "ITEM_DEFINED_TRANSFORMATION")
    })?;
    let ie = entities.get(&idt)?;
    let axes: Vec<u64> = ie.args.iter().filter_map(Value::as_ref_id).collect();
    let (Some(&a1), Some(&a2)) = (axes.first(), axes.get(1)) else {
        return None;
    };
    let m1 = axis_placement_matrix(entities, a1);
    let m2 = axis_placement_matrix(entities, a2);
    Some(crate::cad_import::mat4_mul(&m2, &rigid_inverse(&m1)))
}

/// The inverse of a rigid (rotation + translation, orthonormal basis) column-major 4×4 — exact for an
/// `AXIS2_PLACEMENT_3D` frame (no scale/shear): `R⁻¹ = Rᵀ`, `t⁻¹ = −Rᵀt`.
fn rigid_inverse(m: &[f64; 16]) -> [f64; 16] {
    let t = [m[12], m[13], m[14]];
    [
        m[0],
        m[4],
        m[8],
        0.0, // col0 = Rᵀ row 0
        m[1],
        m[5],
        m[9],
        0.0, // col1
        m[2],
        m[6],
        m[10],
        0.0, // col2
        -(m[0] * t[0] + m[1] * t[1] + m[2] * t[2]),
        -(m[4] * t[0] + m[5] * t[1] + m[6] * t[2]),
        -(m[8] * t[0] + m[9] * t[1] + m[10] * t[2]),
        1.0,
    ]
}

/// Depth-first walk of the product-assembly tree: emit a placed part for each product carrying B-rep
/// geometry, then recurse into child occurrences with the composed transform. Bounded depth + a
/// current-path cycle guard (a malformed self-referential assembly is bounded, never a hang).
#[allow(clippy::too_many_arguments)]
fn walk_brep(
    entities: &BTreeMap<u64, Entity>,
    pd: u64,
    world: [f64; 16],
    depth: u32,
    pd_to_sr: &BTreeMap<u64, Vec<u64>>,
    children: &BTreeMap<u64, Vec<(u64, u64)>>,
    nauo_xform: &BTreeMap<u64, [f64; 16]>,
    colors: &BTreeMap<u64, [f32; 3]>,
    cache: &mut BTreeMap<u64, Option<BrepMesh>>,
    on_path: &mut BTreeSet<u64>,
    reached: &mut BTreeSet<u64>,
    notes: &mut Vec<UnsupportedNote>,
    out: &mut Vec<TessPart>,
) {
    if depth > crate::cad_import::MAX_ASSEMBLY_DEPTH
        || out.len() >= MAX_BREP_PARTS
        || on_path.contains(&pd)
    {
        return;
    }
    reached.insert(pd);
    // This product's own geometry (if any), tessellated once + cached (the product-level notes are pushed
    // exactly once, at cache-fill).
    if let Some(srs) = pd_to_sr.get(&pd) {
        let entry = cache
            .entry(pd)
            .or_insert_with(|| tessellate_product_brep(entities, srs, colors, notes))
            .clone();
        if let Some((mesh, color, name)) = entry {
            out.push(TessPart {
                name,
                reference: pd.to_string(),
                transform: world,
                mesh,
                color,
            });
        }
    }
    on_path.insert(pd);
    if let Some(kids) = children.get(&pd) {
        for &(child, nauo) in kids {
            if out.len() >= MAX_BREP_PARTS {
                break;
            }
            let t = nauo_xform
                .get(&nauo)
                .copied()
                .unwrap_or(crate::cad_import::IDENTITY_4X4);
            let child_world = crate::cad_import::mat4_mul(&world, &t);
            walk_brep(
                entities,
                child,
                child_world,
                depth + 1,
                pd_to_sr,
                children,
                nauo_xform,
                colors,
                cache,
                on_path,
                reached,
                notes,
                out,
            );
        }
    }
    on_path.remove(&pd);
}

/// Tessellate all exact B-rep solids across a product's `SHAPE_REPRESENTATION`s into one welded local
/// mesh (planar faces; curved faces ride the OCCT seam) + its authored colour + a display name. `None`
/// when the reps carry no solid geometry at all (a pure sub-assembly placement node). **Never silent:**
/// a curved-only product (faces present, planar tessellation empty) returns an EMPTY mesh — the pipeline
/// diagnoses + proxies it downstream — and the per-face approximation notes + the construction-artifact
/// filter each land in `notes`, so no real part vanishes without a word.
fn tessellate_product_brep(
    entities: &BTreeMap<u64, Entity>,
    srs: &[u64],
    colors: &BTreeMap<u64, [f32; 3]>,
    notes: &mut Vec<UnsupportedNote>,
) -> Option<BrepMesh> {
    let mut name = String::new();
    let mut faces: Vec<CadFace> = Vec::new();
    let mut color: Option<[f32; 3]> = None;
    let mut sink: Vec<UnsupportedNote> = Vec::new();
    let mut first_sr = 0u64;
    for &sr in srs {
        let Some(e) = entities.get(&sr) else {
            continue;
        };
        if first_sr == 0 {
            first_sr = sr;
        }
        if name.is_empty() {
            if let Some(Value::Str(s)) = e.args.first() {
                if !s.trim().is_empty() {
                    name.clone_from(s);
                }
            }
        }
        // Every item in the shape rep; a geometry item resolves to one or more shells, an axis item to none.
        for item in refs_in(&e.args) {
            let shells = brep_item_shells(entities, item);
            if shells.is_empty() {
                continue;
            }
            color = color.or_else(|| colors.get(&item).copied());
            for shell in shells {
                let Some(sh) = entities.get(&shell) else {
                    continue;
                };
                let Some(face_refs) = sh.args.get(1).and_then(Value::as_list) else {
                    continue;
                };
                for fr in face_refs {
                    if let Some(fid) = fr.as_ref_id() {
                        if let Ok(face) = interpret_face(entities, fid, &mut sink) {
                            faces.push(face);
                        }
                    }
                }
            }
        }
    }
    if name.is_empty() {
        name = format!("solid #{first_sr}");
    }
    if faces.is_empty() {
        return None; // no geometry in any rep — a pure placement node, nothing to show or explain
    }
    // Per-face approximations (curved surfaces the planar tessellator can't cover) are REPORTED, not
    // discarded — one note per product naming the count (the raw sink is one note per face).
    if !sink.is_empty() {
        notes.push(UnsupportedNote {
            feature: format!("B-rep product \"{name}\""),
            detail: format!(
                "{} curved face(s) approximated/skipped by the planar tessellator (exact curved \
                 surfaces ride the OCCT seam)",
                sink.len()
            ),
        });
    }
    let scene = CadScene {
        name: String::new(),
        format: String::new(),
        units: Units {
            meters_per_unit: 0.001,
            kilograms_per_unit: 1.0,
        },
        solids: vec![CadSolid {
            id: first_sr,
            faces,
        }],
        pmi: Vec::new(),
        notes: Vec::new(),
    };
    // Assembly bakes tessellate at PREVIEW grade (quality by context): a 13k-occurrence cell at exact
    // viewer grade measured minutes of registration/persist for invisible gain at factory scale.
    let mesh = scene.tessellate_with(crate::analytic::PREVIEW_DEFLECTION);
    let extent = mesh_axis_extent(&mesh);
    if mesh.triangle_count() > 0 && extent > MAX_PART_EXTENT_MM {
        // The construction-artifact filter (an unbounded reference plane exported as a giant plate) —
        // EXPLAINED, never silent: if this ever catches a real 120 m+ part (a runway rail, a hull block),
        // the note names it and the threshold so the user knows exactly what to re-check.
        notes.push(UnsupportedNote {
            feature: format!("B-rep product \"{name}\""),
            detail: format!(
                "dropped as a construction/reference artifact: its solid spans {extent:.0} mm in \
                 one axis (> the {MAX_PART_EXTENT_MM:.0} mm artifact threshold)"
            ),
        });
        return None;
    }
    // A curved-only product tessellates to 0 triangles — return it EMPTY so the pipeline places a
    // diagnosed proxy (never a silently-vanished part).
    Some((mesh, color, name))
}

/// The largest per-axis extent of a mesh's local bounding box (the construction-artifact filter — a
/// per-axis span, not the diagonal, so a long-but-real beam is kept while a 262 m reference plate is not).
fn mesh_axis_extent(mesh: &TriMesh) -> f64 {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in &mesh.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max)
}

/// The `CLOSED_SHELL` / `OPEN_SHELL` ids a geometry item resolves to — handles `MANIFOLD_SOLID_BREP`,
/// `BREP_WITH_VOIDS`, `FACETED_BREP`, and `SHELL_BASED_SURFACE_MODEL` uniformly by collecting every
/// shell reachable in the item's args (a non-geometry item — an `AXIS2_PLACEMENT_3D` — yields none).
fn brep_item_shells(entities: &BTreeMap<u64, Entity>, item: u64) -> Vec<u64> {
    let Some(e) = entities.get(&item) else {
        return Vec::new();
    };
    let mut refs = Vec::new();
    for a in &e.args {
        collect_refs(a, &mut refs);
    }
    refs.into_iter()
        .filter(|r| {
            entities
                .get(r)
                .is_some_and(|x| x.name == "CLOSED_SHELL" || x.name == "OPEN_SHELL")
        })
        .collect()
}

/// A `(COMPLEX_)TRIANGULATED_FACE` → `(coordinates-list id, 0-based triangle indices INTO that list)`.
/// **Structure-agnostic** (tolerates the AP242 variants): it finds the coordinates by following the arg that
/// references a COORDINATES_LIST, the optional `pnindex` remap as the first bare-int list, and the connectivity
/// as EVERY arg that is a list of integer-lists — a 3-index list is one triangle; a longer list is a triangle
/// STRIP (what COMPLEX_TRIANGULATED_FACE uses), triangulated with alternating winding. Face-local indices are
/// 1-based and remapped through `pnindex` → the (0-based) coordinates list; the caller rebases them into the
/// solid's shared vertex buffer and bounds-checks against the coordinate count.
#[allow(clippy::cast_possible_truncation)]
fn face_triangles(entities: &BTreeMap<u64, Entity>, id: u64) -> Option<(u64, Vec<[u32; 3]>)> {
    let e = entities.get(&id)?;
    if !e.name.contains("TRIANGULATED") {
        return None;
    }
    let coords_id = e.args.iter().find_map(|a| {
        a.as_ref_id().filter(|r| {
            entities
                .get(r)
                .is_some_and(|c| c.name == "COORDINATES_LIST")
        })
    })?;

    // pnindex (optional): the first non-empty list of BARE ints — remaps face-local indices → coordinate
    // indices (both 1-based). A COMPLEX_TRIANGULATED_FACE's strips/fans index into this. We keep its POSITION
    // too: the connectivity attributes are the args that follow pnindex in the entity.
    let pnindex_pos = e.args.iter().position(|a| {
        matches!(a, Value::List(l) if !l.is_empty() && l.iter().all(|v| matches!(v, Value::Int(_))))
    });
    let pnindex: Vec<usize> = pnindex_pos
        .and_then(|p| e.args.get(p))
        .and_then(Value::as_list)
        .map(|l| {
            l.iter()
                .filter_map(|v| match v {
                    Value::Int(i) => usize::try_from(*i).ok(),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let map = |i: i64| -> u32 {
        let one = usize::try_from(i).unwrap_or(0);
        let coord_1b = if !pnindex.is_empty() && one >= 1 && one <= pnindex.len() {
            pnindex[one - 1]
        } else {
            one
        };
        u32::try_from(coord_1b.saturating_sub(1)).unwrap_or(u32::MAX)
    };

    let mut tris: Vec<[u32; 3]> = Vec::new();
    let mut add = |a: u32, b: u32, c: u32| {
        if a != b && b != c && a != c {
            tris.push([a, b, c]);
        }
    };

    // Connectivity — distinguished by POSITION, not shape. A `(complex_)triangulated_face`'s connectivity
    // attributes follow pnindex in schema order `[triangles?, triangle_strips, triangle_fans]`. `triangle_strips`
    // and `triangle_fans` are the SAME list-of-int-lists shape, so a shape-only scan cannot tell them apart —
    // it would fan-triangulate a strip (or vice-versa) and silently emit wrong triangles. So we take the args
    // after pnindex that are connectivity lists (a list of int-lists of len ≥ 3, OR an empty list — an empty
    // `triangle_fans ()` must still hold its slot so a real strips arg is not mis-read as fans), and treat the
    // LAST of ≥2 such args as `triangle_fans` (fan topology: every triangle shares the first vertex). The
    // earlier args (triangles / strips) use strip topology, which also emits a lone len-3 sublist as one
    // triangle — so a plain `triangulated_face` (a single `triangles` arg) is handled correctly too.
    let is_conn = |a: &Value| {
        matches!(a, Value::List(l) if l.is_empty()
            || l.iter().all(|x| matches!(x, Value::List(t) if t.len() >= 3 && t.iter().all(|v| matches!(v, Value::Int(_))))))
    };
    let conn: Vec<usize> = e
        .args
        .iter()
        .enumerate()
        .filter(|(i, a)| pnindex_pos.is_some_and(|p| *i > p) && is_conn(a))
        .map(|(i, _)| i)
        .collect();
    let fans_slot = if conn.len() >= 2 {
        conn.last().copied()
    } else {
        None
    };
    for &ai in &conn {
        let Some(Value::List(list)) = e.args.get(ai) else {
            continue;
        };
        let is_fan = Some(ai) == fans_slot;
        for sub in list {
            let Value::List(idxs) = sub else { continue };
            let seq: Vec<i64> = idxs
                .iter()
                .filter_map(|v| match v {
                    Value::Int(i) => Some(*i),
                    _ => None,
                })
                .collect();
            if seq.len() < 3 {
                continue;
            }
            if is_fan {
                // A triangle fan s0,s1,…: every triangle shares the first vertex → (s0, s_{k+1}, s_{k+2}).
                for k in 0..seq.len() - 2 {
                    add(map(seq[0]), map(seq[k + 1]), map(seq[k + 2]));
                }
            } else {
                // A triangle strip s0,s1,… (a lone len-3 is one triangle): tri k = (sk,sk+1,sk+2), alt winding.
                for k in 0..seq.len() - 2 {
                    let (x, y, z) = if k % 2 == 0 {
                        (seq[k], seq[k + 1], seq[k + 2])
                    } else {
                        (seq[k + 1], seq[k], seq[k + 2])
                    };
                    add(map(x), map(y), map(z));
                }
            }
        }
    }
    if tris.is_empty() {
        return None;
    }
    Some((coords_id, tris))
}

/// A COORDINATES_LIST → its vertices.
fn coordinates_list(entities: &BTreeMap<u64, Entity>, id: u64) -> Option<Vec<[f64; 3]>> {
    let e = entities.get(&id)?;
    if e.name != "COORDINATES_LIST" {
        return None;
    }
    // COORDINATES_LIST(name, npoints, ((x,y,z),(x,y,z),...)) — the points are the last list arg.
    let pts = e.args.iter().rev().find_map(Value::as_list)?;
    let mut out = Vec::with_capacity(pts.len());
    for p in pts {
        if let Some(c) = p.as_list() {
            let x = c.first().and_then(Value::as_real)?;
            let y = c.get(1).and_then(Value::as_real)?;
            let z = c.get(2).and_then(Value::as_real)?;
            out.push([x, y, z]);
        }
    }
    Some(out)
}

/// An AXIS2_PLACEMENT_3D → a column-major rigid 4×4 (orthonormal frame from the z + x-ref directions + the
/// origin point). Missing axis/ref default to +Z / +X.
fn axis_placement_matrix(entities: &BTreeMap<u64, Entity>, id: u64) -> [f64; 16] {
    let Some(e) = entities.get(&id) else {
        return crate::cad_import::IDENTITY_4X4;
    };
    // AXIS2_PLACEMENT_3D(name, #location, #axis(z), #ref_direction(x))
    let origin = e
        .args
        .get(1)
        .and_then(Value::as_ref_id)
        .and_then(|p| point_of(entities, p).ok())
        .unwrap_or([0.0, 0.0, 0.0]);
    let z = e
        .args
        .get(2)
        .and_then(Value::as_ref_id)
        .and_then(|d| direction_of(entities, d))
        .unwrap_or([0.0, 0.0, 1.0]);
    let xref = e
        .args
        .get(3)
        .and_then(Value::as_ref_id)
        .and_then(|d| direction_of(entities, d))
        .unwrap_or([1.0, 0.0, 0.0]);
    let z = normalize(z);
    let x = normalize(sub(xref, scale3(z, dot(xref, z))));
    let y = cross(z, x);
    [
        x[0], x[1], x[2], 0.0, //
        y[0], y[1], y[2], 0.0, //
        z[0], z[1], z[2], 0.0, //
        origin[0], origin[1], origin[2], 1.0,
    ]
}

/// A DIRECTION → its unit-ish vector.
fn direction_of(entities: &BTreeMap<u64, Entity>, id: u64) -> Option<[f64; 3]> {
    let e = entities.get(&id)?;
    if e.name != "DIRECTION" {
        return None;
    }
    let c = e.args.get(1).and_then(Value::as_list)?;
    Some([
        c.first().and_then(Value::as_real)?,
        c.get(1).and_then(Value::as_real)?,
        c.get(2).and_then(Value::as_real)?,
    ])
}

// ── small vector / rigid-matrix helpers ──────────────────────────────────────────────────────────────────
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale3(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt();
    if l > 1e-12 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}
// ============================================================================================
// Faceted re-export (geometry preserved; NURBS not — the OCCT seam)
// ============================================================================================

/// Format an f64 round-trippably (17 significant digits) so a re-import recovers the exact coordinate.
fn real(x: f64) -> String {
    // STEP reals need a decimal point; `{:?}` on f64 is the shortest round-trippable form and always
    // includes a point or exponent for a real. Ensure a trailing point for whole numbers.
    let s = format!("{x:?}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.")
    }
}

/// Export a [`CadScene`] to a valid ISO-10303-21 faceted B-rep (POLY_LOOP) text. Geometry (vertices +
/// planar faces) is preserved within the round-trip tolerance; curved faces are dropped with a header note
/// (the honest downgrade — full round-trip of NURBS is the OCCT seam).
#[allow(clippy::format_push_string)] // a small one-shot serializer; readability over write! churn
fn export_faceted(scene: &CadScene) -> Result<String, StepError> {
    let mut out = String::new();
    out.push_str("ISO-10303-21;\n");
    out.push_str("HEADER;\n");
    out.push_str("FILE_DESCRIPTION(('Metrocalk faceted re-export'),'2;1');\n");
    out.push_str(&format!(
        "FILE_NAME('{}','',(''),(''),'metrocalk-interchange','','');\n",
        scene.name.replace('\'', "''")
    ));
    out.push_str("FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\n");
    out.push_str("ENDSEC;\n");
    out.push_str("DATA;\n");

    let mut id: u64 = 0;
    let mut next = || {
        id += 1;
        id
    };

    // Weld vertices → CARTESIAN_POINT ids.
    let mut pt_ids: BTreeMap<[u64; 3], u64> = BTreeMap::new();
    let mut point_id = |p: [f64; 3], out: &mut String, next: &mut dyn FnMut() -> u64| -> u64 {
        let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
        if let Some(&i) = pt_ids.get(&key) {
            return i;
        }
        let i = next();
        out.push_str(&format!(
            "#{i} = CARTESIAN_POINT('',({},{},{}));\n",
            real(p[0]),
            real(p[1]),
            real(p[2])
        ));
        pt_ids.insert(key, i);
        i
    };

    let mut face_ids: Vec<u64> = Vec::new();
    // original CadFace.id → its emitted FACE #id, so a PMI shape_aspect points at the re-emitted face.
    let mut emitted_face: BTreeMap<u64, u64> = BTreeMap::new();
    let mut n_curved = 0usize;
    for solid in &scene.solids {
        for face in &solid.faces {
            if face.kind != FaceKind::Planar || face.outer.len() < 3 {
                n_curved += 1;
                continue;
            }
            let loop_pts: Vec<u64> = face
                .outer
                .iter()
                .map(|&p| point_id(p, &mut out, &mut next))
                .collect();
            let loop_refs = loop_pts
                .iter()
                .map(|i| format!("#{i}"))
                .collect::<Vec<_>>()
                .join(",");
            let loop_id = next();
            out.push_str(&format!("#{loop_id} = POLY_LOOP('',({loop_refs}));\n"));
            let bound_id = next();
            out.push_str(&format!(
                "#{bound_id} = FACE_OUTER_BOUND('',#{loop_id},.T.);\n"
            ));
            // A faceted-b-rep FACE (no surface entity — the polygon is planar by construction).
            let f = next();
            out.push_str(&format!("#{f} = FACE('',(#{bound_id}));\n"));
            face_ids.push(f);
            emitted_face.insert(face.id, f);
        }
    }

    if face_ids.is_empty() {
        return Err(StepError::Empty(
            "no planar faces to export (all curved — that round-trip is the OCCT seam)".into(),
        ));
    }
    let shell_refs = face_ids
        .iter()
        .map(|i| format!("#{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let shell = next();
    out.push_str(&format!("#{shell} = CLOSED_SHELL('',({shell_refs}));\n"));
    let brep = next();
    out.push_str(&format!("#{brep} = FACETED_BREP('',#{shell});\n"));

    if n_curved > 0 {
        out.push_str(&format!(
            "/* {n_curved} curved face(s) omitted from this faceted re-export — full NURBS round-trip is \
             the OpenCascade native/server seam (ADR-070) */\n"
        ));
    }

    // Emit the semantic PMI (AP242 geometric_tolerance entities) — machine-readable, never a graphical
    // downgrade (M15.5 / ADR-075). Any PMI that can't round-trip (curved-face reference / unknown
    // characteristic) is an explained comment, never a silent drop.
    let mut pmi_notes: Vec<String> = Vec::new();
    export_pmi_entities(scene, &mut out, &mut next, &emitted_face, &mut pmi_notes);
    for n in &pmi_notes {
        out.push_str(&format!("/* {n} */\n"));
    }

    out.push_str("ENDSEC;\n");
    out.push_str("END-ISO-10303-21;\n");
    Ok(out)
}

// ============================================================================================
// Round-trip fidelity (the declared, measured tolerance budget)
// ============================================================================================

/// The measured round-trip deviation: the largest nearest-point distance between the original scene's
/// welded vertices and the re-imported scene's welded vertices. A **planar** part round-trips within the
/// coordinate-formatting budget (declared below); curved faces are excluded (the OCCT seam).
#[must_use]
pub fn round_trip_deviation(before: &CadScene, after: &CadScene) -> f64 {
    let va = welded_vertices(before);
    let vb = welded_vertices(after);
    let mut max_dev = 0.0f64;
    for p in &va {
        let mut best = f64::INFINITY;
        for q in &vb {
            let d = dist2(*p, *q);
            if d < best {
                best = d;
            }
        }
        max_dev = max_dev.max(best.sqrt());
    }
    max_dev
}

/// The declared exchange tolerance budget for the planar/faceted round-trip: with 17-sig-digit
/// round-trippable f64 formatting, planar geometry re-imports **exactly**, so the budget is a tight
/// 1e-6 (scene units) — the honest number we publish (never "lossless").
pub const ROUND_TRIP_BUDGET: f64 = 1e-6;

fn welded_vertices(scene: &CadScene) -> Vec<[f64; 3]> {
    let mut set: BTreeMap<[u64; 3], [f64; 3]> = BTreeMap::new();
    for solid in &scene.solids {
        for face in &solid.faces {
            if face.kind != FaceKind::Planar {
                continue;
            }
            for &p in &face.outer {
                set.insert([p[0].to_bits(), p[1].to_bits(), p[2].to_bits()], p);
            }
        }
    }
    set.into_values().collect()
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

// ============================================================================================
// Semantic PMI — AP242 GD&T round-trip (M15.5 / ADR-075), a DECLARED SUBSET
// ============================================================================================
//
// We read/write the AP242 **semantic** geometric_tolerance entity chain so a feature-control-frame survives
// the round-trip **still semantic** (a typed characteristic + a numeric zone + a face/datum reference —
// machine-readable), NOT downgraded to a **graphical** callout (a drawn annotation a human reads). The
// honest bound (measured, not badged): a **declared subset** — the 10 form/orientation/location
// characteristics (M15.3) on a **single datum**, with the simplifications that (1) the standard rides the
// geometric_tolerance `description`, (2) the toleranced/datum shape_aspect references the face directly
// rather than through the full product_definition_shape + geometric_item_specific_usage chain. Full AP242
// ed4 conformance (the complex-instance datum_system algebra, MMC/LMC/composite frames) + wild-vendor
// fidelity is the **OCCT-backed native/server seam** (ADR-070). Our own writer emits only semantic entities,
// so a round-trip **through this crate** is 100% semantic on the declared subset — the fidelity we publish.

/// The bijection between the editor's canonical GD&T characteristic token and the AP242 `geometric_tolerance`
/// subtype entity name (ISO 10303-242). `circularity` maps to `ROUNDNESS_TOLERANCE` (the STEP spelling).
const GDT_MAP: [(&str, &str); 10] = [
    ("flatness", "FLATNESS_TOLERANCE"),
    ("straightness", "STRAIGHTNESS_TOLERANCE"),
    ("circularity", "ROUNDNESS_TOLERANCE"),
    ("cylindricity", "CYLINDRICITY_TOLERANCE"),
    ("parallelism", "PARALLELISM_TOLERANCE"),
    ("perpendicularity", "PERPENDICULARITY_TOLERANCE"),
    ("angularity", "ANGULARITY_TOLERANCE"),
    ("position", "POSITION_TOLERANCE"),
    ("concentricity", "CONCENTRICITY_TOLERANCE"),
    ("symmetry", "SYMMETRY_TOLERANCE"),
];

/// The AP242 `geometric_tolerance` subtype entity name for a canonical GD&T token (e.g. `position` →
/// `POSITION_TOLERANCE`). `None` if the token is not one of the declared-subset characteristics.
#[must_use]
pub fn gdt_entity_name(token: &str) -> Option<&'static str> {
    GDT_MAP.iter().find(|(t, _)| *t == token).map(|(_, e)| *e)
}

/// The canonical GD&T token for an AP242 `geometric_tolerance` subtype entity name (the inverse of
/// [`gdt_entity_name`]). `None` if the entity is not a recognized declared-subset tolerance.
#[must_use]
pub fn gdt_token(entity_name: &str) -> Option<&'static str> {
    GDT_MAP
        .iter()
        .find(|(_, e)| *e == entity_name)
        .map(|(t, _)| *t)
}

/// Resolve a `LENGTH_MEASURE_WITH_UNIT` (or a bare `LENGTH_MEASURE`) `#id` → its millimetre value.
fn measure_value(entities: &BTreeMap<u64, Entity>, id: u64) -> Option<f64> {
    let e = entities.get(&id)?;
    // LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(<v>), #unit) — arg 0 is the typed measure.
    match e.args.first() {
        Some(Value::Typed(_, inner)) => inner.first().and_then(Value::as_real),
        Some(v) => v.as_real(),
        None => None,
    }
}

/// Resolve a `SHAPE_ASPECT` `#id` → the referenceable face `#id` it is bound to (the arg that is a `#ref`).
fn shape_aspect_face(entities: &BTreeMap<u64, Entity>, id: u64) -> Option<u64> {
    let e = entities.get(&id)?;
    if e.name != "SHAPE_ASPECT" {
        return None;
    }
    // SHAPE_ASPECT(name, description, #of_shape, product_definitional) — the face ref is the first #ref arg.
    e.args.iter().find_map(Value::as_ref_id)
}

/// Resolve a `DATUM` `#id` → its datum face `#id` (via its `SHAPE_ASPECT`).
fn datum_face(entities: &BTreeMap<u64, Entity>, id: u64) -> Option<u64> {
    let e = entities.get(&id)?;
    if e.name != "DATUM" {
        return None;
    }
    // DATUM(name, description, #shape_aspect, product_definitional, identification) — follow the shape_aspect.
    let sa = e.args.iter().find_map(Value::as_ref_id)?;
    shape_aspect_face(entities, sa)
}

/// Pull one [`CadPmi`] from a `GEOMETRIC_TOLERANCE` record's args `(name, description, #magnitude,
/// #toleranced_shape_aspect)` + an optional datum ref. `face_ids` gates the face reference to a real
/// resolved face (never a dangle).
fn pmi_from_gt(
    entities: &BTreeMap<u64, Entity>,
    gt_args: &[Value],
    characteristic: &str,
    datum_ref: Option<u64>,
    face_ids: &std::collections::BTreeSet<u64>,
) -> Option<CadPmi> {
    let standard = match gt_args.get(1) {
        Some(Value::Str(s)) => s.clone(),
        _ => String::new(),
    };
    let value_mm = gt_args
        .get(2)
        .and_then(Value::as_ref_id)
        .and_then(|m| measure_value(entities, m))?;
    let sa = gt_args.get(3).and_then(Value::as_ref_id)?;
    let face_id = shape_aspect_face(entities, sa)?;
    if !face_ids.contains(&face_id) {
        return None;
    }
    let datum_face_id = datum_ref
        .and_then(|d| datum_face(entities, d))
        .filter(|d| face_ids.contains(d));
    Some(CadPmi {
        face_id,
        characteristic: characteristic.to_string(),
        value_mm,
        datum_face_id,
        standard,
        semantic: true,
    })
}

/// Scan the entity graph for AP242 semantic PMI (`geometric_tolerance` subtypes, simple or complex instance)
/// and interpret each into a [`CadPmi`]. A **graphical-only** callout (`*ANNOTATION*` / `DRAUGHTING_CALLOUT`)
/// that is *not* backed by a semantic tolerance is **not** surfaced as PMI — it is an explained note (the
/// honest downgrade: a graphic is not machine-readable; the semantic path is what round-trips). Deterministic
/// order (the entity map is a `BTreeMap`).
fn parse_pmi(
    entities: &BTreeMap<u64, Entity>,
    face_ids: &std::collections::BTreeSet<u64>,
    notes: &mut Vec<UnsupportedNote>,
) -> Vec<CadPmi> {
    let mut pmi = Vec::new();
    let mut graphical = 0usize;
    for (id, e) in entities {
        // A simple-instance form tolerance: `FLATNESS_TOLERANCE(name, description, #mag, #tsa)`.
        if let Some(token) = gdt_token(&e.name) {
            if let Some(p) = pmi_from_gt(entities, &e.args, token, None, face_ids) {
                pmi.push(p);
            }
            continue;
        }
        // A complex-instance datum-referencing tolerance:
        //   `(GEOMETRIC_TOLERANCE(...) GEOMETRIC_TOLERANCE_WITH_DATUM_REFERENCE((#dat)) <LEAF>())`.
        if e.name == COMPLEX_INSTANCE {
            let sub = |n: &str| {
                e.args.iter().find_map(|a| match a {
                    Value::Typed(name, inner) if name == n => Some(inner.as_slice()),
                    _ => None,
                })
            };
            let leaf_token = e.args.iter().find_map(|a| match a {
                Value::Typed(name, _) => gdt_token(name),
                _ => None,
            });
            if let (Some(token), Some(gt)) = (leaf_token, sub("GEOMETRIC_TOLERANCE")) {
                let datum_ref = sub("GEOMETRIC_TOLERANCE_WITH_DATUM_REFERENCE")
                    .and_then(|d| d.first())
                    .and_then(Value::as_list)
                    .and_then(|l| l.first())
                    .and_then(Value::as_ref_id);
                if let Some(p) = pmi_from_gt(entities, gt, token, datum_ref, face_ids) {
                    pmi.push(p);
                }
            }
            continue;
        }
        // A graphical-only annotation (a drawn callout) — counted + explained, NOT surfaced as semantic PMI.
        if e.name.contains("ANNOTATION") || e.name == "DRAUGHTING_CALLOUT" {
            graphical += 1;
        }
        let _ = id;
    }
    if graphical > 0 {
        notes.push(UnsupportedNote {
            feature: format!("{graphical} graphical PMI callout(s)"),
            detail: "a drawn annotation is NOT machine-readable — not surfaced as semantic PMI. Recovering \
                     semantic tolerances from graphics-only PMI is the OCCT / full-AP242 native/server seam \
                     (ADR-070/075)."
                .into(),
        });
    }
    pmi
}

/// Emit the AP242 semantic-PMI entities for a scene's [`CadScene::pmi`], into the DATA section of a faceted
/// re-export. `emitted_face`: original `CadFace.id` → its emitted `FACE` `#id` (so the shape_aspect points at
/// the re-emitted face). A PMI whose face wasn't emitted (curved → OCCT seam) is skipped with a note.
#[allow(clippy::format_push_string)]
fn export_pmi_entities(
    scene: &CadScene,
    out: &mut String,
    next: &mut dyn FnMut() -> u64,
    emitted_face: &BTreeMap<u64, u64>,
    notes: &mut Vec<String>,
) {
    if scene.pmi.is_empty() {
        return;
    }
    // One shared millimetre length unit.
    let unit = next();
    out.push_str(&format!("#{unit} = SI_UNIT(.MILLI.,.METRE.);\n"));

    for p in &scene.pmi {
        let Some(entity_name) = gdt_entity_name(&p.characteristic) else {
            notes.push(format!(
                "PMI '{}' on face #{} — unknown characteristic, not exported (semantic downgrade)",
                p.characteristic, p.face_id
            ));
            continue;
        };
        let Some(&face) = emitted_face.get(&p.face_id) else {
            notes.push(format!(
                "PMI '{}' references face #{} which is not in the faceted export (curved → OCCT seam)",
                p.characteristic, p.face_id
            ));
            continue;
        };

        let mag = next();
        out.push_str(&format!(
            "#{mag} = LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE({}),#{unit});\n",
            real(p.value_mm)
        ));
        let fsa = next();
        out.push_str(&format!(
            "#{fsa} = SHAPE_ASPECT('{}','metrocalk-semantic-pmi',#{face},.T.);\n",
            p.characteristic
        ));

        let std_tok = p.standard.replace('\'', "''");
        if let Some(dface) = p.datum_face_id.and_then(|d| emitted_face.get(&d).copied()) {
            // A datum-referencing tolerance → the faithful AP242 complex (AND-combined) instance.
            let dsa = next();
            out.push_str(&format!(
                "#{dsa} = SHAPE_ASPECT('datum','metrocalk-semantic-pmi',#{dface},.T.);\n"
            ));
            let dat = next();
            out.push_str(&format!(
                "#{dat} = DATUM('A','datum feature',#{dsa},.T.,'A');\n"
            ));
            let tol = next();
            out.push_str(&format!(
                "#{tol} = (GEOMETRIC_TOLERANCE('{}','{}',#{mag},#{fsa})\
                 GEOMETRIC_TOLERANCE_WITH_DATUM_REFERENCE((#{dat}))\
                 {entity_name}());\n",
                p.characteristic, std_tok
            ));
        } else {
            if let Some(missing) = p.datum_face_id {
                notes.push(format!(
                    "PMI '{}' datum face #{missing} not in the faceted export (curved → OCCT seam); \
                     exported datumless",
                    p.characteristic,
                ));
            }
            // A datumless form tolerance → a conformant simple instance.
            let tol = next();
            out.push_str(&format!(
                "#{tol} = {entity_name}('{}','{}',#{mag},#{fsa});\n",
                p.characteristic, std_tok
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrocalk_csg::validate;

    /// A real ADVANCED_BREP cube (2×2×2 mm, centred at origin) in ISO-10303-21 / AP242 form: 8
    /// CARTESIAN_POINTs, 8 VERTEX_POINTs, 12 EDGE_CURVEs, 6 ADVANCED_FACEs over PLANEs, one CLOSED_SHELL —
    /// exactly the topology chain a CAD tool exports. Hand-authored for the spike (disclosed); the format
    /// is standard-conformant and any STEP reader parses it.
    const CUBE_STEP: &str = include_str!("../tests/fixtures/cube_ap242.step");

    #[test]
    fn a_real_advanced_brep_cube_imports_with_referenceable_faces_and_edges() {
        let scene = StepInterchange
            .import(CUBE_STEP.as_bytes())
            .expect("import");
        assert_eq!(scene.solids.len(), 1, "one solid");
        assert_eq!(scene.face_count(), 6, "a cube has 6 referenceable faces");
        // Each face is a quad with 4 referenceable edges.
        assert!(
            scene.solids[0].faces.iter().all(|f| f.edges.len() == 4),
            "each face has 4 referenceable edges"
        );
        // Faces carry stable STEP #ids (the M15.3 PMI hook).
        assert!(scene.solids[0].faces.iter().all(|f| f.id > 0));
    }

    #[test]
    fn the_cube_tessellates_watertight() {
        let scene = StepInterchange
            .import(CUBE_STEP.as_bytes())
            .expect("import");
        let mesh = scene.tessellate();
        let r = validate(&mesh);
        assert!(
            r.watertight && r.manifold,
            "the tessellated cube is watertight+manifold: {}",
            r.explain()
        );
        assert_eq!(r.genus, Some(0), "a cube is genus 0");
        assert_eq!(mesh.triangle_count(), 12, "6 quads → 12 triangles");
    }

    #[test]
    fn round_trip_is_within_the_declared_tolerance_budget() {
        let step = StepInterchange;
        let before = step.import(CUBE_STEP.as_bytes()).expect("import");
        let exported = step.export(&before).expect("re-export");
        let after = step.import(exported.as_bytes()).expect("re-import");
        let dev = round_trip_deviation(&before, &after);
        assert!(
            dev <= ROUND_TRIP_BUDGET,
            "round-trip deviation {dev:e} <= budget {ROUND_TRIP_BUDGET:e}"
        );
        // The re-export is itself valid + watertight.
        assert!(validate(&after.tessellate()).watertight);
    }

    #[test]
    fn malformed_inputs_are_explained_never_panic() {
        let step = StepInterchange;
        // Not a STEP file at all.
        assert!(matches!(
            step.import(b"just some bytes"),
            Err(StepError::Malformed(_))
        ));
        // Truncated (no END wrapper).
        assert!(step
            .import(b"ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#1 = ")
            .is_err());
        // Dangling ref: a shell that points at a non-existent face.
        let dangling = "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#1 = CLOSED_SHELL('',(#999));\nENDSEC;\nEND-ISO-10303-21;\n";
        assert!(matches!(
            step.import(dangling.as_bytes()),
            Err(StepError::DanglingRef(999))
        ));
        // Oversized.
        let big = vec![b'x'; MAX_STEP_BYTES + 1];
        assert!(matches!(step.import(&big), Err(StepError::TooLarge { .. })));
        // A valid wrapper but no B-rep → Empty, explained.
        let empty = "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#1 = CARTESIAN_POINT('',(0.,0.,0.));\nENDSEC;\nEND-ISO-10303-21;\n";
        assert!(matches!(
            step.import(empty.as_bytes()),
            Err(StepError::Empty(_))
        ));
    }

    #[test]
    fn deeply_nested_input_is_bounded_never_a_stack_overflow() {
        // A crafted deep-nesting statement (`#1 = A(((…0…)))`, within MAX_STEP_BYTES) would recurse to a
        // stack-overflow ABORT without the depth guard. It must be an explained StepError, never a panic —
        // the M10.2 never-panic gate on adversarial input (the M15.5 hardening). 300 > MAX_PAREN_DEPTH (256),
        // so the guard fires while the recursion is still shallow (no real overflow risk in this test).
        let deep = format!("A{}0{}", "(".repeat(300), ")".repeat(300));
        let s = format!(
            "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#1 = {deep};\nENDSEC;\nEND-ISO-10303-21;\n"
        );
        match StepInterchange.import(s.as_bytes()) {
            Err(StepError::Malformed(why)) => assert!(
                why.contains("nesting"),
                "the deep-nesting guard explains it: {why}"
            ),
            other => panic!("expected a Malformed nesting error, got {other:?}"),
        }
    }

    #[test]
    fn a_curved_surface_is_referenced_and_explained_not_dropped() {
        // A face over a CYLINDRICAL_SURFACE is kept as a referenceable Curved face + an explained note
        // (the OCCT seam), never silently lost.
        let s = "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n\
            #1 = CARTESIAN_POINT('',(0.,0.,0.));\n\
            #2 = CARTESIAN_POINT('',(1.,0.,0.));\n\
            #3 = CARTESIAN_POINT('',(1.,1.,0.));\n\
            #4 = VERTEX_POINT('',#1);\n\
            #5 = VERTEX_POINT('',#2);\n\
            #6 = VERTEX_POINT('',#3);\n\
            #7 = EDGE_CURVE('',#4,#5,$,.T.);\n\
            #8 = EDGE_CURVE('',#5,#6,$,.T.);\n\
            #9 = EDGE_CURVE('',#6,#4,$,.T.);\n\
            #10 = ORIENTED_EDGE('',*,*,#7,.T.);\n\
            #11 = ORIENTED_EDGE('',*,*,#8,.T.);\n\
            #12 = ORIENTED_EDGE('',*,*,#9,.T.);\n\
            #13 = EDGE_LOOP('',(#10,#11,#12));\n\
            #14 = FACE_OUTER_BOUND('',#13,.T.);\n\
            #15 = CYLINDRICAL_SURFACE('',$,1.);\n\
            #16 = ADVANCED_FACE('',(#14),#15,.T.);\n\
            #17 = CLOSED_SHELL('',(#16));\n\
            ENDSEC;\nEND-ISO-10303-21;\n";
        let scene = StepInterchange.import(s.as_bytes()).expect("import");
        assert_eq!(scene.face_count(), 1, "the curved face is still referenced");
        assert_eq!(scene.solids[0].faces[0].kind, FaceKind::Curved);
        assert!(
            scene.notes.iter().any(|n| n.detail.contains("OpenCascade")),
            "the OCCT seam is explained, not silent"
        );
    }

    // ── M15.8 (ADR-078): analytic curved-surface tessellation — smooth, deterministic, kernel-free ─────────

    /// A real cylinder wall (radius 5, height 10, z axis) as a full ADVANCED_FACE over a placed
    /// CYLINDRICAL_SURFACE — the bore/boss shape every machined part carries.
    const CYL_STEP: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\n\
        FILE_NAME('cyl','',(''),(''),'','','');\nFILE_SCHEMA(('AP242'));\nENDSEC;\nDATA;\n\
        #1 = CARTESIAN_POINT('',(0.,0.,0.));\n\
        #2 = DIRECTION('',(0.,0.,1.));\n\
        #3 = DIRECTION('',(1.,0.,0.));\n\
        #4 = AXIS2_PLACEMENT_3D('',#1,#2,#3);\n\
        #5 = CARTESIAN_POINT('',(5.,0.,0.));\n\
        #6 = CARTESIAN_POINT('',(5.,0.,10.));\n\
        #7 = VERTEX_POINT('',#5);\n\
        #8 = VERTEX_POINT('',#6);\n\
        #9 = EDGE_CURVE('',#7,#7,$,.T.);\n\
        #10 = EDGE_CURVE('',#7,#8,$,.T.);\n\
        #11 = EDGE_CURVE('',#8,#8,$,.T.);\n\
        #12 = ORIENTED_EDGE('',*,*,#9,.T.);\n\
        #13 = ORIENTED_EDGE('',*,*,#10,.T.);\n\
        #14 = ORIENTED_EDGE('',*,*,#11,.T.);\n\
        #15 = EDGE_LOOP('',(#12,#13,#14));\n\
        #16 = FACE_OUTER_BOUND('',#15,.T.);\n\
        #17 = CYLINDRICAL_SURFACE('',#4,5.);\n\
        #18 = ADVANCED_FACE('',(#16),#17,.T.);\n\
        #19 = CLOSED_SHELL('',(#18));\n\
        #20 = MANIFOLD_SOLID_BREP('cyl',#19);\n\
        ENDSEC;\nEND-ISO-10303-21;\n";

    #[test]
    fn an_analytic_cylinder_tessellates_smooth_not_faceted_and_deterministic() {
        let scene = StepInterchange.import(CYL_STEP.as_bytes()).expect("import");
        let face = &scene.solids[0].faces[0];
        assert_eq!(
            face.kind,
            FaceKind::Curved,
            "still a referenceable curved face"
        );
        let surface = face
            .surface
            .expect("the cylinder is RECOGNIZED, not the kernel seam");
        assert!(
            !scene.notes.iter().any(|n| n.feature.contains("face #18")),
            "a handled analytic face carries no seam note: {:?}",
            scene.notes
        );

        // It TESSELLATES — smooth real geometry, not a skip (the old behavior) and not a facet or two.
        let mesh = scene.tessellate();
        assert!(
            mesh.triangle_count() >= 32,
            "adaptive full revolution, got {} triangles",
            mesh.triangle_count()
        );
        // Every wall vertex sits exactly on the cylinder (closed-form, no sag beyond fp noise).
        for p in &mesh.positions {
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
            assert!((r - 5.0).abs() < 1e-9, "vertex off the wall: r={r}");
            assert!((-1e-9..=10.0 + 1e-9).contains(&p[2]), "outside the height");
        }
        // THE SMOOTHNESS GATE ("a faceted cylinder is a FAIL"): facet normals within the deflection-derived
        // bound of the exact analytic normal.
        let patch =
            crate::analytic::tessellate_analytic(18, &surface, &face.outer, face.same_sense)
                .expect("plan validated at parse time");
        let dev = crate::analytic::max_normal_deviation(&patch, &surface);
        let bound = 2.0 * (1.0 - crate::analytic::DEFLECTION / 5.0).acos();
        assert!(
            dev <= bound.max(0.5),
            "faceted: worst facet-normal deviation {dev} rad"
        );

        // DETERMINISTIC: same file → bit-identical mesh hash, ×3 (the regression-corpus property).
        let h = crate::cad_import::mesh_hash(&mesh);
        for _ in 0..3 {
            let again = StepInterchange
                .import(CYL_STEP.as_bytes())
                .expect("re-import")
                .tessellate();
            assert_eq!(
                crate::cad_import::mesh_hash(&again),
                h,
                "tessellation drifted"
            );
        }
    }

    // ── M15.5 (ADR-075): AP242 semantic-PMI round-trip through the pure-Rust Part-21 subset ────────────────

    /// The cube imported, with two semantic FCFs attached to its faces (a datum-referencing position + a
    /// datumless flatness). The face ids come from the real import.
    fn cube_with_pmi() -> CadScene {
        let mut scene = StepInterchange
            .import(CUBE_STEP.as_bytes())
            .expect("import");
        let f: Vec<u64> = scene.solids[0].faces.iter().map(|face| face.id).collect();
        scene.pmi = vec![
            CadPmi {
                face_id: f[0],
                characteristic: "position".into(),
                value_mm: 0.10,
                datum_face_id: Some(f[1]),
                standard: "ASME_Y14.5".into(),
                semantic: true,
            },
            CadPmi {
                face_id: f[2],
                characteristic: "flatness".into(),
                value_mm: 0.02,
                datum_face_id: None,
                standard: "ISO_GPS".into(),
                semantic: true,
            },
        ];
        scene
    }

    #[test]
    fn semantic_pmi_round_trips_as_machine_readable_structured_data() {
        let step = StepInterchange;
        let before = cube_with_pmi();
        let exported = step.export(&before).expect("re-export with PMI");
        // The exported STEP carries SEMANTIC geometric_tolerance entities, not graphical callouts.
        assert!(
            exported.contains("POSITION_TOLERANCE"),
            "position is semantic"
        );
        assert!(
            exported.contains("FLATNESS_TOLERANCE"),
            "flatness is semantic"
        );
        assert!(exported.contains("GEOMETRIC_TOLERANCE_WITH_DATUM_REFERENCE"));
        assert!(!exported.contains("ANNOTATION"), "no graphical downgrade");

        let after = step
            .import(exported.as_bytes())
            .expect("re-import with PMI");
        assert_eq!(after.pmi.len(), 2, "both FCFs survive the round-trip");

        // The position FCF: still semantic, value + datum-presence + standard preserved, on a real face.
        let pos = after
            .pmi
            .iter()
            .find(|p| p.characteristic == "position")
            .expect("position survived semantic");
        assert!(pos.semantic, "still SEMANTIC, not graphical");
        assert!((pos.value_mm - 0.10).abs() < 1e-12, "value bit-preserved");
        assert!(pos.datum_face_id.is_some(), "datum reference preserved");
        assert_eq!(pos.standard, "ASME_Y14.5", "standard preserved");
        let face_ids: std::collections::BTreeSet<u64> = after
            .solids
            .iter()
            .flat_map(|s| &s.faces)
            .map(|f| f.id)
            .collect();
        assert!(face_ids.contains(&pos.face_id), "attached to a real face");

        // The flatness FCF: datumless form tolerance survives semantic.
        let flat = after
            .pmi
            .iter()
            .find(|p| p.characteristic == "flatness")
            .expect("flatness survived semantic");
        assert!(flat.semantic && flat.datum_face_id.is_none());
        assert!((flat.value_mm - 0.02).abs() < 1e-12);
        assert_eq!(flat.standard, "ISO_GPS");

        // Geometry still round-trips within budget (PMI didn't perturb the vertices).
        assert!(round_trip_deviation(&before, &after) <= ROUND_TRIP_BUDGET);
    }

    #[test]
    #[allow(clippy::cast_precision_loss)] // i is 0..9 — the usize→f64 cast is exact
    fn all_ten_declared_characteristics_round_trip_semantic() {
        let step = StepInterchange;
        let mut scene = StepInterchange
            .import(CUBE_STEP.as_bytes())
            .expect("import");
        let f: Vec<u64> = scene.solids[0].faces.iter().map(|face| face.id).collect();
        // Attach every declared characteristic; orientation/location get a datum, form does not.
        let datum = |t: &str| {
            matches!(
                t,
                "parallelism"
                    | "perpendicularity"
                    | "angularity"
                    | "position"
                    | "concentricity"
                    | "symmetry"
            )
        };
        for (i, (token, _)) in GDT_MAP.iter().enumerate() {
            scene.pmi.push(CadPmi {
                face_id: f[i % f.len()],
                characteristic: (*token).into(),
                value_mm: 0.01 * (i as f64 + 1.0),
                datum_face_id: datum(token).then(|| f[(i + 1) % f.len()]),
                standard: "ASME_Y14.5".into(),
                semantic: true,
            });
        }
        let exported = step.export(&scene).expect("export all 10");
        let after = step.import(exported.as_bytes()).expect("re-import all 10");
        assert_eq!(after.pmi.len(), 10, "all 10 characteristics round-trip");
        for (token, _) in GDT_MAP {
            assert!(
                after
                    .pmi
                    .iter()
                    .any(|p| p.characteristic == token && p.semantic),
                "{token} survived semantic"
            );
        }
    }

    #[test]
    fn a_graphical_only_callout_is_noted_not_surfaced_as_semantic() {
        // A file whose PMI is a GRAPHICAL annotation (a drawn callout) — NOT a geometric_tolerance. Our
        // reader must NOT surface it as semantic PMI; it's an explained downgrade note (the honest boundary).
        let mut scene = cube_with_pmi();
        scene.pmi.clear();
        let mut exported = step_export_no_pmi(&scene);
        // Splice a graphical annotation before ENDSEC.
        exported = exported.replace(
            "ENDSEC;\nEND-ISO-10303-21;\n",
            "#9001 = ANNOTATION_OCCURRENCE('drawn callout',$,$);\nENDSEC;\nEND-ISO-10303-21;\n",
        );
        let after = StepInterchange.import(exported.as_bytes()).expect("import");
        assert!(
            after.pmi.is_empty(),
            "a graphical callout is NOT semantic PMI"
        );
        assert!(
            after
                .notes
                .iter()
                .any(|n| n.detail.contains("machine-readable")),
            "the graphical downgrade is explained, not silent"
        );
    }

    fn step_export_no_pmi(scene: &CadScene) -> String {
        StepInterchange.export(scene).expect("export")
    }
}
