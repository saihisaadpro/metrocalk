//! The **CATIA 3DXML reader** (M15.7 / ADR-077) — the native tier that reads the exact file that
//! black-screened Unreal. A `.3dxml` is a **ZIP of XML**:
//! - `PRODUCT.3dxml` — the **readable** product structure: `Reference3D` (unique parts) · `Instance3D`
//!   (occurrences, each with a `RelativeMatrix` transform) · `ReferenceRep`→`.3DRep` (the geometry files) ·
//!   `InstanceRep`. We parse this fully → the assembly graph → **every occurrence accounted for + placed at
//!   its real transform** (never-silent + never-empty).
//! - `.3DRep` — the representation files. When **open XML tessellation** (`<Rep>`/`PolygonalRepType`), we
//!   parse + render it instantly (the "like a texture" cache-first core). When **proprietary CATIA binary**
//!   (`V5_CFV3`/`CATIA_V5`/`CB0001` — the CGR-family cache the CGM kernel owns), we place a **proxy at the
//!   real assembly transform + diagnose it** (the licensed-kernel/re-export fix path) — the honest boundary,
//!   never a silent 0-triangle shell.
//! - a `.PLMBriefcase` (the proprietary PLM payload) — **not read** (it is the licensed-kernel domain).
//!
//! Native-only (behind the `3dxml` feature): `zip::`/`quick_xml::` are confined to this module (invariant 5,
//! CI grep-gated). Bounds-checked throughout — an oversized/corrupt/cyclic/deeply-nested file is an explained
//! [`CadError`] or a diagnosed proxy, **never a panic**.

use super::{
    build_import, mat4_mul, source_hash, CadError, CadImport, CadReader, GroupNode, PartSource,
    RawPart, IDENTITY_4X4, MAX_ASSEMBLY_DEPTH,
};
use crate::{Units, UnsupportedNote};
use metrocalk_csg::TriMesh;
use quick_xml::events::Event;
use quick_xml::reader::Reader as XmlReader;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use zip::ZipArchive;

/// Reject a `.3dxml` larger than this (the decode-bomb guard; the real bar file is ~222 MB).
const MAX_3DXML_BYTES: usize = 1024 * 1024 * 1024;
/// Cap the decompressed size of any single XML/rep entry we read (never inflate an entry-bomb unbounded).
const MAX_ENTRY_BYTES: u64 = 128 * 1024 * 1024;
/// Cap the number of product-structure elements parsed (a `#id` bomb).
const MAX_ELEMENTS: usize = 8_000_000;
/// Cap the number of leaf-part occurrences emitted by the assembly walk (an occurrence-graph bomb).
const MAX_PARTS: usize = 4_000_000;

/// The CATIA 3DXML reader. See the module docs.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThreeDxmlReader;

impl CadReader for ThreeDxmlReader {
    fn format(&self) -> &'static str {
        "CATIA-3DXML"
    }

    fn can_read(&self, bytes: &[u8]) -> bool {
        // A ZIP (the definitive 3DXML validation — Manifest/PRODUCT.3dxml — happens in `read`, so a non-3DXML
        // ZIP returns a clean `Unrecognized`, never a mis-import).
        bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06")
    }

    fn read(&self, bytes: &[u8]) -> Result<CadImport, CadError> {
        read_3dxml(bytes)
    }
}

// ============================================================================================
// The parsed product structure (from PRODUCT.3dxml)
// ============================================================================================

/// One assembly edge — an `Instance3D` occurrence of a `Reference3D` under a parent.
struct InstEdge {
    inst_id: u64,
    instance_of: u64,
    matrix: [f64; 16],
}

/// The parsed product structure — enough to walk the assembly and resolve each occurrence's geometry.
#[derive(Default)]
struct ProductStructure {
    root: u64,
    /// `Reference3D` id → display name.
    ref_name: BTreeMap<u64, String>,
    /// `Reference3D` id → its child `Instance3D` occurrences (document order → deterministic walk).
    children: BTreeMap<u64, Vec<InstEdge>>,
    /// `Reference3D` id → the `ReferenceRep` id(s) that carry its geometry (via `InstanceRep`s). A part can
    /// aggregate MORE THAN ONE rep (a multi-body part, or an exact + tessellated rep, or LOD reps) — ALL are
    /// kept + emitted, so a multi-rep reference is never silently reduced to its last rep.
    ref_geom: BTreeMap<u64, Vec<u64>>,
    /// `ReferenceRep` id → (display name, the `.3DRep` zip-entry name).
    refrep: BTreeMap<u64, (String, String)>,
    /// The count of `Instance3D` occurrences (the "1,280 parts" headline number).
    n_instances: usize,
}

/// A minimal owned accumulator for the element currently being parsed.
#[derive(Default)]
struct Current {
    kind: ElemKind,
    id: u64,
    name: String,
    assoc_file: String,
    aggregated_by: u64,
    instance_of: u64,
    matrix: Option<[f64; 16]>,
    v_name: String,
}

#[derive(Default, PartialEq, Clone, Copy)]
enum ElemKind {
    #[default]
    None,
    Reference3D,
    Instance3D,
    ReferenceRep,
    InstanceRep,
}

/// Build a fresh element context from a target element's attributes (its text children fill the rest).
fn begin_element(name: &[u8], e: &quick_xml::events::BytesStart) -> Current {
    Current {
        kind: match name {
            b"Reference3D" => ElemKind::Reference3D,
            b"Instance3D" => ElemKind::Instance3D,
            b"ReferenceRep" => ElemKind::ReferenceRep,
            _ => ElemKind::InstanceRep,
        },
        id: attr_u64(e, b"id").unwrap_or(0),
        name: attr_string(e, b"name"),
        assoc_file: attr_string(e, b"associatedFile"),
        ..Default::default()
    }
}

/// Parse `PRODUCT.3dxml` into the assembly graph (streaming; bounds-checked; never panics).
#[allow(clippy::too_many_lines)] // one streaming event loop — cohesive, not decomposable without obscuring
fn parse_product_structure(xml: &str) -> Result<ProductStructure, CadError> {
    let mut reader = XmlReader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut ps = ProductStructure::default();
    let mut cur = Current::default();
    let mut field: Option<Vec<u8>> = None; // the current text-child tag inside an element
    let mut count = 0usize;

    loop {
        match reader.read_event() {
            Err(e) => {
                return Err(CadError::Malformed(format!(
                    "PRODUCT.3dxml XML error at {}: {e}",
                    reader.buffer_position()
                )))
            }
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let local = e.local_name();
                let name = local.as_ref();
                match name {
                    b"ProductStructure" => {
                        if let Some(r) = attr_u64(&e, b"root") {
                            ps.root = r;
                        }
                    }
                    b"Reference3D" | b"Instance3D" | b"ReferenceRep" | b"InstanceRep" => {
                        count += 1;
                        if count > MAX_ELEMENTS {
                            return Err(CadError::TooLarge(format!(
                                "product structure exceeds {MAX_ELEMENTS} elements"
                            )));
                        }
                        cur = begin_element(name, &e); // its text children fill it; committed on End
                        field = None;
                    }
                    _ => {
                        // A text-child tag inside an element (IsAggregatedBy / IsInstanceOf / RelativeMatrix /
                        // V_Name / …) — remember it so the following Text routes to the right field.
                        if cur.kind != ElemKind::None {
                            field = Some(name.to_vec());
                        }
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                // A self-closing element carries no children — commit a target element straight from its
                // attributes (so a `<ReferenceRep ... />` with no V_Name child is NEVER silently lost).
                let local = e.local_name();
                let name = local.as_ref();
                if let b"Reference3D" | b"Instance3D" | b"ReferenceRep" | b"InstanceRep" = name {
                    count += 1;
                    if count > MAX_ELEMENTS {
                        return Err(CadError::TooLarge(format!(
                            "product structure exceeds {MAX_ELEMENTS} elements"
                        )));
                    }
                    commit(&mut ps, &begin_element(name, &e));
                } else if name == b"ProductStructure" {
                    if let Some(r) = attr_u64(&e, b"root") {
                        ps.root = r;
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if cur.kind == ElemKind::None {
                    continue;
                }
                let Some(tag) = field.as_deref() else {
                    continue;
                };
                let text = t.unescape().unwrap_or_default();
                let text = text.trim();
                match tag {
                    b"IsAggregatedBy" => cur.aggregated_by = text.parse().unwrap_or(0),
                    b"IsInstanceOf" => cur.instance_of = text.parse().unwrap_or(0),
                    b"V_Name" => cur.v_name = text.to_string(),
                    b"RelativeMatrix" => cur.matrix = parse_matrix(text),
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let local = e.local_name();
                let name = local.as_ref();
                match name {
                    b"Reference3D" | b"Instance3D" | b"ReferenceRep" | b"InstanceRep" => {
                        commit(&mut ps, &cur);
                        cur = Current::default();
                        field = None;
                    }
                    _ => {
                        // Closing a text-child tag.
                        if field.as_deref() == Some(name) {
                            field = None;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if ps.root == 0 && !ps.ref_name.contains_key(&0) {
        // No explicit root attr — fall back to the smallest Reference3D id (best-effort, deterministic).
        if let Some((&min, _)) = ps.ref_name.iter().next() {
            ps.root = min;
        }
    }
    if ps.ref_name.is_empty() {
        return Err(CadError::Unrecognized(
            "PRODUCT.3dxml has no Reference3D — not a 3DXML product structure".into(),
        ));
    }
    Ok(ps)
}

/// Commit a finished element into the product structure.
fn commit(ps: &mut ProductStructure, cur: &Current) {
    match cur.kind {
        ElemKind::Reference3D => {
            let name = if cur.v_name.is_empty() {
                cur.name.clone()
            } else {
                cur.v_name.clone()
            };
            ps.ref_name.insert(cur.id, name);
        }
        ElemKind::Instance3D => {
            ps.n_instances += 1;
            ps.children
                .entry(cur.aggregated_by)
                .or_default()
                .push(InstEdge {
                    inst_id: cur.id,
                    instance_of: cur.instance_of,
                    matrix: cur.matrix.unwrap_or(IDENTITY_4X4),
                });
        }
        ElemKind::ReferenceRep => {
            let name = if cur.v_name.is_empty() {
                cur.name.clone()
            } else {
                cur.v_name.clone()
            };
            let entry = cur
                .assoc_file
                .rsplit(':')
                .next()
                .unwrap_or(&cur.assoc_file)
                .to_string();
            ps.refrep.insert(cur.id, (name, entry));
        }
        ElemKind::InstanceRep => {
            // Links a Reference3D (aggregated_by) → a ReferenceRep (instance_of) = the ref's geometry. A ref
            // may have several — keep them ALL (never drop all-but-the-last).
            ps.ref_geom
                .entry(cur.aggregated_by)
                .or_default()
                .push(cur.instance_of);
        }
        ElemKind::None => {}
    }
}

// ============================================================================================
// The reader: open the ZIP → parse the structure → sniff reps → walk the assembly
// ============================================================================================

/// What a `.3DRep` entry resolved to when sniffed.
enum RepSniff {
    /// Proprietary CATIA binary (the licensed-kernel seam) — the detected encoding label.
    Proprietary(String),
    /// Open XML tessellation, parsed to a mesh (the cache-first "like a texture" path).
    Open(TriMesh),
    /// The referenced entry is missing from the archive.
    Missing,
}

#[allow(clippy::too_many_lines)] // one linear orchestration: open → parse → sniff → walk → notes
fn read_3dxml(bytes: &[u8]) -> Result<CadImport, CadError> {
    if bytes.len() > MAX_3DXML_BYTES {
        return Err(CadError::TooLarge(format!(
            "{} bytes > {MAX_3DXML_BYTES} cap",
            bytes.len()
        )));
    }
    let mut zip = ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| CadError::Malformed(format!("not a valid ZIP/3DXML container: {e}")))?;

    // The root product-structure doc — via Manifest.xml (<Root>…</Root>), else the conventional PRODUCT.3dxml.
    let root_doc = manifest_root(&mut zip).unwrap_or_else(|| "PRODUCT.3dxml".to_string());
    let product_xml = read_entry_string(&mut zip, &root_doc)
        .or_else(|| read_entry_string(&mut zip, "PRODUCT.3dxml"))
        .ok_or_else(|| {
            CadError::Unrecognized(
                "no Manifest.xml / PRODUCT.3dxml product structure — not a 3DXML".into(),
            )
        })?;

    let ps = parse_product_structure(&product_xml)?;

    // Sniff each unique referenced .3DRep ONCE (proprietary vs open-XML vs missing).
    let mut sniff: BTreeMap<String, RepSniff> = BTreeMap::new();
    let referenced: BTreeSet<String> = ps
        .ref_geom
        .values()
        .flatten()
        .filter_map(|rr| ps.refrep.get(rr).map(|(_, e)| e.clone()))
        .collect();
    for entry in &referenced {
        let s = sniff_rep(&mut zip, entry);
        sniff.insert(entry.clone(), s);
    }

    // Walk the assembly **forest**, composing transforms, emitting a RawPart per geometry-bearing occurrence
    // (never-silent) with its resolved source (open tessellation / proprietary proxy / missing). A 3DXML can
    // carry several disconnected product trees — the declared root names one; a never-drop importer shows
    // them ALL. The forest roots are the references never instanced (aggregated) by anyone (plus the declared
    // root); each is walked at identity, sorted for determinism.
    let instanced: BTreeSet<u64> = ps
        .children
        .values()
        .flatten()
        .map(|e| e.instance_of)
        .collect();
    let mut roots: Vec<u64> = ps
        .ref_name
        .keys()
        .copied()
        .filter(|r| !instanced.contains(r))
        .collect();
    if ps.ref_name.contains_key(&ps.root) && !roots.contains(&ps.root) {
        roots.push(ps.root); // an (unusual) instanced declared root is still a display top
    }
    roots.sort_unstable();

    let mut raw_parts: Vec<RawPart> = Vec::new();
    let mut groups: Vec<GroupNode> = Vec::new();
    let mut visits: u64 = 0;
    for &r in &roots {
        let mut on_path: BTreeSet<u64> = BTreeSet::new();
        on_path.insert(r);
        walk(
            r,
            IDENTITY_4X4,
            0,
            path_combine(ROOT_PATH_HASH, r),
            None, // a forest root has no parent group
            &ps,
            &sniff,
            &mut raw_parts,
            &mut groups,
            &mut on_path,
            &mut visits,
        );
    }

    // Never-drop fallback: any geometry reference STILL unreached (a disconnected cycle with no forest root)
    // is placed at the origin + surfaced — never a silent drop (multi-rep + dangling reps handled by
    // emit_geometry). Deterministic order (BTreeMap).
    let reached: BTreeSet<u64> = raw_parts
        .iter()
        .filter_map(|p| p.reference.parse::<u64>().ok())
        .collect();
    let mut placed_at_origin = 0usize;
    for &ref_id in ps.ref_geom.keys() {
        if reached.contains(&ref_id) || raw_parts.len() >= MAX_PARTS {
            continue;
        }
        if emit_geometry(
            ref_id,
            path_combine(ROOT_PATH_HASH, ref_id),
            IDENTITY_4X4,
            None, // a disconnected reference has no assembly parent — a top-level part
            &ps,
            &sniff,
            &mut raw_parts,
        ) {
            placed_at_origin += 1;
        }
    }

    let name = ps
        .ref_name
        .get(&ps.root)
        .cloned()
        .unwrap_or_else(|| "CATIA assembly".to_string());

    // Units: 3DXML V6 is millimetres by convention (the RelativeMatrix translations are in mm). Add a
    // bounding-box sanity note (the 10× trap backstop) computed from the placed part positions.
    let mut notes: Vec<UnsupportedNote> = Vec::new();
    if let Some(diag) = bbox_diagonal(&raw_parts) {
        // A plausible mechanical assembly spans ~mm..tens-of-metres (1..1e7 mm). Outside → flag, never silently
        // wrong-scale.
        let mm = diag; // scene units are mm
        if !(1.0..=1.0e7).contains(&mm) {
            notes.push(UnsupportedNote {
                feature: "unit/scale sanity".into(),
                detail: format!(
                    "the placed assembly spans {mm:.1} scene-units (mm) — outside the plausible 1 mm..10 km \
                     range; verify the source units (the 10× trap backstop)"
                ),
            });
        }
    }
    if raw_parts.len() >= MAX_PARTS {
        notes.push(UnsupportedNote {
            feature: "occurrence cap".into(),
            detail: format!(
                "the assembly walk hit the {MAX_PARTS}-occurrence cap — some deeply-instanced occurrences \
                 were not expanded (bounded, never a hang)"
            ),
        });
    }
    if visits > MAX_WALK_VISITS {
        notes.push(UnsupportedNote {
            feature: "assembly-graph bound".into(),
            detail: format!(
                "the assembly walk hit the {MAX_WALK_VISITS}-visit bound (a deeply-branching / diamond-DAG \
                 structure) — bounded + surfaced, never a hang; some occurrences were not expanded"
            ),
        });
    }
    if groups.len() >= MAX_PARTS {
        notes.push(UnsupportedNote {
            feature: "structural-node cap".into(),
            detail: format!(
                "the assembly walk hit the {MAX_PARTS}-structural-node cap — subtrees beyond it were not \
                 expanded (bounded, never a hang; unreached geometry is origin-placed + reported below)"
            ),
        });
    }
    if ps.ref_geom.is_empty() {
        notes.push(UnsupportedNote {
            feature: "no geometry".into(),
            detail: "the product structure has references but NO geometry-bearing rep — an all-structural \
                     assembly (nothing to place); reported, not a silent empty result"
                .into(),
        });
    }
    notes.push(UnsupportedNote {
        feature: "CATIA proprietary geometry".into(),
        detail: "exact geometry for proprietary V5_CFV3/CB0001 .3DRep parts is the licensed-kernel seam \
                 (Spatial 3D InterOp / HOOPS Exchange) behind the CadReader trait, or a STEP AP242 re-export \
                 — every such part is placed at its real transform + diagnosed, never a silent empty shell"
            .into(),
    });

    // NEVER-SILENT completeness: every geometry-bearing reference is now placed — reached by the forest walk,
    // or by the origin fallback above. Surface any origin-placed count so the accounting stays honest.
    if placed_at_origin > 0 {
        notes.push(UnsupportedNote {
            feature: "geometry outside the assembly tree".into(),
            detail: format!(
                "{placed_at_origin} geometry reference(s) had no placement in the instance tree (a \
                 disconnected component) — placed at the origin + reported, never silently dropped"
            ),
        });
    }

    let mut imp = build_import(
        name,
        "CATIA-3DXML".into(),
        Units {
            meters_per_unit: 0.001,
            kilograms_per_unit: 1.0,
        },
        source_hash(bytes),
        raw_parts,
        groups,
        notes,
    );
    // The honest headline numbers: the raw source instance count (the research's "1,280") + the number of
    // top-level products (forest roots) shown.
    imp.total_occurrences = ps.n_instances;
    imp.products = roots.len();
    Ok(imp)
}

/// The FNV seed for occurrence path-hashing (stable, unique-per-occurrence part ids robust to re-import).
const ROOT_PATH_HASH: u64 = 0xcbf2_9ce4_8422_2325;

/// Combine a path hash with the next instance id (FNV-1a) — the occurrence id encodes root→…→this instance,
/// so it is stable across re-imports and unique per occurrence (two occurrences of one reference differ).
fn path_combine(h: u64, inst_id: u64) -> u64 {
    let mut h = h ^ inst_id;
    h = h.wrapping_mul(0x0000_0100_0000_01b3);
    h
}

/// A global bound on the total number of `walk()` node visits across the whole forest — independent of the
/// emitted-part cap. The cycle guard only prevents a reference twice on the CURRENT path, so a crafted
/// **acyclic diamond DAG** with no geometry (Rk aggregates 2 instances of R(k+1)) produces 2^k visits while
/// emitting zero parts (so `MAX_PARTS` never fires). This cap turns that from an exponential hang into an
/// explained bound (a `TooLarge`-style note), never a panic/hang on adversarial input.
const MAX_WALK_VISITS: u64 = 16_000_000;

/// Emit a `RawPart` for EACH `ReferenceRep` a geometry-bearing reference carries — a multi-body/LOD part with
/// several reps is fully accounted for, and a **dangling** ReferenceRep id (marked geometry-bearing but never
/// defined) is placed as a diagnosed `Missing` proxy, not silently skipped. Returns true if the reference
/// carried any geometry link.
fn emit_geometry(
    ref_id: u64,
    path_hash: u64,
    world: [f64; 16],
    parent: Option<u64>,
    ps: &ProductStructure,
    sniff: &BTreeMap<String, RepSniff>,
    out: &mut Vec<RawPart>,
) -> bool {
    let Some(reps) = ps.ref_geom.get(&ref_id) else {
        return false;
    };
    for &rr in reps {
        if out.len() >= MAX_PARTS {
            break;
        }
        // A distinct, stable per-rep id when a single occurrence carries several reps (else keep the plain
        // occurrence path-hash, so 1:1 files — the common case — keep identical ids across re-imports).
        let id = if reps.len() > 1 {
            path_combine(path_hash, rr)
        } else {
            path_hash
        };
        let (name, source) = match ps.refrep.get(&rr) {
            Some((rep_name, entry)) => {
                let name = if rep_name.is_empty() {
                    ps.ref_name.get(&ref_id).cloned().unwrap_or_default()
                } else {
                    rep_name.clone()
                };
                let source = match sniff.get(entry) {
                    Some(RepSniff::Open(mesh)) => PartSource::Tessellation(mesh.clone()),
                    Some(RepSniff::Proprietary(enc)) => PartSource::ProprietaryRep {
                        encoding: enc.clone(),
                    },
                    Some(RepSniff::Missing) | None => PartSource::Missing {
                        detail: format!("3DRep '{entry}'"),
                    },
                };
                (name, source)
            }
            None => (
                ps.ref_name.get(&ref_id).cloned().unwrap_or_default(),
                PartSource::Missing {
                    detail: format!("dangling ReferenceRep #{rr}"),
                },
            ),
        };
        out.push(RawPart {
            id,
            name,
            reference: ref_id.to_string(),
            transform: world,
            source,
            // 3DXML per-part colour (SurfaceAttributes/Color) is a follow-up; the proprietary-rep proxy uses
            // the viewer default for now.
            color: None,
            // The named assembly occurrence (GroupNode) this part nests under — preserves the source tree.
            parent,
        });
    }
    true
}

/// A fixed salt mixed into an occurrence's path hash to derive its [`GroupNode`] id, keeping the structural
/// container distinct from its own leaf part ids (the bare path hash / `path_combine(path_hash, rep_id)`) in
/// the id→entity map. Chosen far above any plausible source rep id so the (probabilistic, like the whole
/// path-hash scheme) collision floor is the same 64-bit guarantee the leaf ids already rely on.
const GROUP_ID_SALT: u64 = 0xA55E_3B19_6D01_0007;

/// The stable id of the [`GroupNode`] for an assembly occurrence with the given path hash.
fn occurrence_group_id(path_hash: u64) -> u64 {
    path_combine(path_hash, GROUP_ID_SALT)
}

/// Depth-first assembly walk: compose transforms, emit a part per geometry-bearing occurrence, AND build the
/// **named structural tree** — each assembly occurrence (a reference WITH children) becomes a [`GroupNode`]
/// and every leaf part / child group records it as `parent`, so the source's exact hierarchy + grouping +
/// names are preserved. Approach: identity organizational groups + world-composed leaf transforms, so
/// placement stays byte-identical to the flat walk (the group carries no transform). `visits` is the global
/// diamond-DAG work bound; a walk beyond [`MAX_WALK_VISITS`] stops (surfaced as a note), never hangs.
#[allow(clippy::too_many_arguments)]
fn walk(
    ref_id: u64,
    world: [f64; 16],
    depth: u32,
    path_hash: u64,
    parent_group: Option<u64>,
    ps: &ProductStructure,
    sniff: &BTreeMap<String, RepSniff>,
    out: &mut Vec<RawPart>,
    groups: &mut Vec<GroupNode>,
    on_path: &mut BTreeSet<u64>,
    visits: &mut u64,
) {
    *visits += 1;
    if depth > MAX_ASSEMBLY_DEPTH
        || out.len() >= MAX_PARTS
        || groups.len() >= MAX_PARTS
        || *visits > MAX_WALK_VISITS
    {
        return;
    }
    let has_children = ps.children.get(&ref_id).is_some_and(|c| !c.is_empty());
    // A reference WITH children is an assembly occurrence → materialize a NAMED group container; its own
    // geometry + child occurrences nest under it. A pure leaf (no children) needs no self-group — its geometry
    // nests directly under the enclosing parent group (no redundant "Bolt › Bolt" wrapping).
    let group_for_children = if has_children {
        let gid = occurrence_group_id(path_hash);
        let name = ps.ref_name.get(&ref_id).cloned().unwrap_or_default();
        groups.push(GroupNode {
            id: gid,
            name,
            parent: parent_group,
        });
        Some(gid)
    } else {
        parent_group
    };
    // This reference's own geometry (a ref can be both a sub-assembly and carry rep(s)).
    emit_geometry(ref_id, path_hash, world, group_for_children, ps, sniff, out);
    // Recurse into sub-assembly children.
    if let Some(children) = ps.children.get(&ref_id) {
        for edge in children {
            if *visits > MAX_WALK_VISITS {
                break;
            }
            if on_path.contains(&edge.instance_of) {
                continue; // cycle guard — never revisit a reference already on the current path
            }
            on_path.insert(edge.instance_of);
            let child_world = mat4_mul(&world, &edge.matrix);
            walk(
                edge.instance_of,
                child_world,
                depth + 1,
                path_combine(path_hash, edge.inst_id),
                group_for_children,
                ps,
                sniff,
                out,
                groups,
                on_path,
                visits,
            );
            on_path.remove(&edge.instance_of);
        }
    }
}

/// Read `Manifest.xml`'s `<Root>…</Root>` — the name of the root product-structure document.
fn manifest_root(zip: &mut ZipArchive<Cursor<&[u8]>>) -> Option<String> {
    let xml = read_entry_string(zip, "Manifest.xml")?;
    // Cheap extract of <Root>…</Root> (avoids a full parse for one element).
    let start = xml.find("<Root>")? + "<Root>".len();
    let end = xml[start..].find("</Root>")? + start;
    Some(xml[start..end].trim().to_string())
}

/// Sniff a `.3DRep` entry: proprietary CATIA binary vs open XML tessellation vs missing.
fn sniff_rep(zip: &mut ZipArchive<Cursor<&[u8]>>, entry: &str) -> RepSniff {
    let Some(bytes) = read_entry_bytes(zip, entry) else {
        return RepSniff::Missing;
    };
    let head = &bytes[..bytes.len().min(64)];
    // CATIA proprietary rep magic (the CGR-family cache — the licensed-kernel seam).
    if head.starts_with(b"V5_CFV3")
        || contains(head, b"CATIA_V5")
        || contains(head, b"CB0001")
        || contains(head, b"V5_CGR")
    {
        return RepSniff::Proprietary("CATIA V5_CFV3/CB0001 proprietary rep".into());
    }
    // Open XML tessellation (some 3DXML exports carry it) — parse + render instantly.
    if head.first().is_some_and(|&b| b == b'<') || head.starts_with(b"\xEF\xBB\xBF<") {
        if let Some(mesh) = parse_open_3drep(&bytes) {
            if mesh.triangle_count() > 0 {
                return RepSniff::Open(mesh);
            }
        }
        // XML but not a tessellation schema we read → treat as the kernel seam, never a silent drop.
        return RepSniff::Proprietary("3DXML XML rep (unrecognized tessellation schema)".into());
    }
    RepSniff::Proprietary("unrecognized .3DRep encoding".into())
}

/// Parse an **open XML 3DRep** (the 3DXML `PolygonalRepType` tessellation) into a mesh: `<Positions>` (a
/// flat list of `x y z` / `x,y,z` floats) + `<Face triangles="i j k …">` index lists. A faithful subset of
/// the real 3DXML tessellation schema — the "show the embedded cache instantly" path.
fn parse_open_3drep(bytes: &[u8]) -> Option<TriMesh> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut reader = XmlReader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut positions: Vec<[f64; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut in_positions = false;

    loop {
        match reader.read_event() {
            Err(_) => return None,
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                if e.local_name().as_ref() == b"Positions" {
                    in_positions = true;
                }
                if e.local_name().as_ref() == b"Face" {
                    push_face_triangles(&e, &mut triangles);
                }
            }
            Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"Face" {
                    push_face_triangles(&e, &mut triangles);
                }
            }
            Ok(Event::Text(t)) if in_positions => {
                let s = t.unescape().unwrap_or_default();
                let nums: Vec<f64> = s
                    .split([' ', ',', '\n', '\t', '\r'])
                    .filter(|x| !x.is_empty())
                    .filter_map(|x| x.parse::<f64>().ok())
                    .collect();
                for c in nums.chunks_exact(3) {
                    positions.push([c[0], c[1], c[2]]);
                    if positions.len() > 50_000_000 {
                        return None; // bomb guard
                    }
                }
            }
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() == b"Positions" {
                    in_positions = false;
                }
            }
            _ => {}
        }
    }
    // Validate the indices reference real vertices (a malformed rep is dropped → the proxy path, not a panic).
    let n = u32::try_from(positions.len()).ok()?;
    if positions.is_empty() || triangles.is_empty() || triangles.iter().flatten().any(|&i| i >= n) {
        return None;
    }
    Some(TriMesh::new(positions, triangles))
}

/// Pull a `<Face triangles="i j k …">` attribute into triangle indices.
fn push_face_triangles(e: &quick_xml::events::BytesStart, triangles: &mut Vec<[u32; 3]>) {
    let Some(v) = attr_raw(e, b"triangles") else {
        return;
    };
    let idx: Vec<u32> = String::from_utf8_lossy(&v)
        .split([' ', ',', '\n', '\t', '\r'])
        .filter(|x| !x.is_empty())
        .filter_map(|x| x.parse::<u32>().ok())
        .collect();
    for c in idx.chunks_exact(3) {
        triangles.push([c[0], c[1], c[2]]);
    }
}

// ── ZIP entry readers (bounds-checked) + small helpers ───────────────────────────────────────────────────

/// Read a ZIP entry fully into bytes, capped at [`MAX_ENTRY_BYTES`]. `None` if the entry is absent/oversized.
fn read_entry_bytes(zip: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let mut f = zip.by_name(name).ok()?;
    if f.size() > MAX_ENTRY_BYTES {
        return None;
    }
    let mut buf = Vec::with_capacity(f.size().min(1024 * 1024) as usize);
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Read a ZIP entry as a UTF-8 string (lossy — a stray non-UTF-8 byte never fails the whole import).
fn read_entry_string(zip: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<String> {
    read_entry_bytes(zip, name).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// `true` if `hay` contains `needle`.
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// A `<... 12 floats ...>` RelativeMatrix → a column-major 4×4 (first 9 = the 3 basis columns, last 3 = the
/// origin — the 3DXML convention). `None` if not exactly 12 numbers.
fn parse_matrix(text: &str) -> Option<[f64; 16]> {
    let n: Vec<f64> = text
        .split([' ', ',', '\n', '\t', '\r'])
        .filter(|x| !x.is_empty())
        .filter_map(|x| x.parse::<f64>().ok())
        .collect();
    if n.len() != 12 {
        return None;
    }
    Some([
        n[0], n[1], n[2], 0.0, // col 0 (x-axis)
        n[3], n[4], n[5], 0.0, // col 1 (y-axis)
        n[6], n[7], n[8], 0.0, // col 2 (z-axis)
        n[9], n[10], n[11], 1.0, // col 3 (origin)
    ])
}

/// The bounding-box diagonal of the placed part positions (for the unit/scale backstop). `None` if empty.
fn bbox_diagonal(parts: &[RawPart]) -> Option<f64> {
    if parts.is_empty() {
        return None;
    }
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in parts {
        let t = [p.transform[12], p.transform[13], p.transform[14]];
        for k in 0..3 {
            lo[k] = lo[k].min(t[k]);
            hi[k] = hi[k].max(t[k]);
        }
    }
    let d = [(hi[0] - lo[0]), (hi[1] - lo[1]), (hi[2] - lo[2])];
    Some((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt())
}

fn attr_raw(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<Vec<u8>> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == key)
        .map(|a| a.value.into_owned())
}

fn attr_u64(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<u64> {
    attr_raw(e, key).and_then(|v| String::from_utf8_lossy(&v).trim().parse().ok())
}

fn attr_string(e: &quick_xml::events::BytesStart, key: &[u8]) -> String {
    attr_raw(e, key).map_or_else(String::new, |v| String::from_utf8_lossy(&v).into_owned())
}

#[cfg(test)]
mod tests;
