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
use std::collections::BTreeMap;

/// Reject a STEP text larger than this before parsing (the M10.2 size cap; mirrors `assets::MAX_IMPORT_BYTES`).
pub const MAX_STEP_BYTES: usize = 64 * 1024 * 1024;
/// Reject a file with more entity instances than this (the decode-bomb guard — a Part-21 file can name
/// millions of `#id`s; cap before allocating the graph).
pub const MAX_ENTITIES: usize = 4_000_000;

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
}

/// One solid body (a `CLOSED_SHELL` / `MANIFOLD_SOLID_BREP`).
#[derive(Clone, PartialEq, Debug)]
pub struct CadSolid {
    /// The STEP entity id of the shell.
    pub id: u64,
    /// The solid's faces.
    pub faces: Vec<CadFace>,
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
    #[allow(clippy::cast_precision_loss)] // polygon vertex counts are tiny
    pub fn tessellate(&self) -> TriMesh {
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
struct Entity {
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

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(s: &'a str) -> Self {
        Cursor {
            bytes: s.as_bytes(),
            pos: 0,
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
    /// Parse a `(...)` list at the cursor into a Vec of Values.
    fn parse_paren_list(&mut self) -> Result<Vec<Value>, StepError> {
        self.skip_ws();
        if self.peek() != Some(b'(') {
            return Err(StepError::Malformed("expected '('".into()));
        }
        self.pos += 1;
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b')') => {
                    self.pos += 1;
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

/// Parse the whole file text and interpret the planar B-rep + faceted subset into a [`CadScene`].
fn parse_and_interpret(text: &str) -> Result<CadScene, StepError> {
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

    interpret(&entities)
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
fn interpret(entities: &BTreeMap<u64, Entity>) -> Result<CadScene, StepError> {
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

    // Classify the surface: PLANE (or a faceted FACE with no surface entity) → tessellated here; any
    // named curved surface → referenced but the OCCT seam.
    let kind = match surface_id.and_then(|sid| entities.get(&sid)) {
        Some(s) if s.name == "PLANE" => FaceKind::Planar,
        Some(s) => {
            notes.push(UnsupportedNote {
                feature: format!("{} on face #{fid}", s.name),
                detail: "curved/freeform surface — referenced (M15.3 PMI can attach) but NOT tessellated \
                         here; exact tessellation is the OpenCascade native/server seam (ADR-070)"
                    .into(),
            });
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

    Ok(CadFace {
        id: fid,
        kind,
        outer,
        edges,
    })
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
fn file_name(entities: &BTreeMap<u64, Entity>) -> Option<String> {
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
}
