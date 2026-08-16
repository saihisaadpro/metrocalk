//! Procedural scatter prototypes — so a new terrain has trees on it, not an empty field.
//!
//! ## Why generate these rather than ship assets
//!
//! A preset that declares "woodland, 110 trees per hectare" and then places nothing because the project has
//! no tree asset is a preset that teaches the author the feature is broken. Shipping binary assets would
//! solve it and cost a licence question, a repository of megabytes, and an import step before anything
//! works. Generating them costs a few hundred lines and gives something better: the meshes are
//! **deterministic**, **content-addressed** like any other asset, come with their own LOD chain and
//! impostor, and can be replaced by real art at any time — the recipe references a handle, and swapping the
//! handle is the whole migration.
//!
//! These are honest stand-ins, not art. A conifer is a stack of cones on a trunk; a broadleaf is a lobed
//! canopy. They read correctly in a landscape at the distances scatter actually draws them, and their
//! silhouettes are what the impostor bake needs. Nothing here pretends to be a hero asset.
//!
//! Same discipline as the rest of the crate: no RNG, no transcendentals in anything that affects geometry —
//! ring positions come from a fixed unit-circle table, so a generated tree is byte-identical everywhere.

use crate::mesh::MeshData;

/// The kinds of stand-in a preset can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtoKind {
    /// A broadleaf tree: trunk plus two lobed canopy layers.
    BroadleafTree,
    /// A conifer: trunk plus stacked cone tiers.
    Conifer,
    /// A low rounded shrub.
    Shrub,
    /// A grass tuft — a few crossed blades, the cheapest thing in the world.
    GrassTuft,
    /// An irregular boulder.
    Boulder,
    /// A desert shrub: sparse, spiky, sun-bleached.
    DesertShrub,
    /// A palm: a bare trunk with a crown of fronds.
    Palm,
}

impl ProtoKind {
    /// The name a preset shows for this stand-in.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::BroadleafTree => "Broadleaf Tree",
            Self::Conifer => "Conifer",
            Self::Shrub => "Shrub",
            Self::GrassTuft => "Grass Tuft",
            Self::Boulder => "Boulder",
            Self::DesertShrub => "Desert Shrub",
            Self::Palm => "Palm",
        }
    }

    /// A stable key fragment, so the host can name the asset it stores.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::BroadleafTree => "broadleaf",
            Self::Conifer => "conifer",
            Self::Shrub => "shrub",
            Self::GrassTuft => "grass",
            Self::Boulder => "boulder",
            Self::DesertShrub => "desert-shrub",
            Self::Palm => "palm",
        }
    }
}

/// A generated stand-in: geometry plus the two colours it is painted with.
///
/// Two colours rather than one because every one of these is a dark woody part under a lighter organic
/// part, and a single flat colour is what makes procedural vegetation read as plastic.
#[derive(Clone, Debug, PartialEq)]
pub struct ProtoMesh {
    /// The geometry, origin at the base, +Y up.
    pub data: MeshData,
    /// Linear RGB for the trunk/stem part.
    pub stem_color: [f32; 3],
    /// Linear RGB for the canopy/foliage part.
    pub canopy_color: [f32; 3],
    /// Index of the first vertex belonging to the canopy; everything before it is stem.
    pub canopy_start: u32,
    /// Approximate horizontal radius in metres.
    pub radius_m: f32,
    /// Approximate height in metres.
    pub height_m: f32,
}

/// Unit-circle directions, tabulated so no trigonometry is involved (crate rule: the geometry of a
/// generated asset must be bit-identical on every machine, because its content address depends on it).
const RING8: [[f32; 2]; 8] = [
    [1.0, 0.0],
    [0.707_106_77, 0.707_106_77],
    [0.0, 1.0],
    [-0.707_106_77, 0.707_106_77],
    [-1.0, 0.0],
    [-0.707_106_77, -0.707_106_77],
    [0.0, -1.0],
    [0.707_106_77, -0.707_106_77],
];

const RING6: [[f32; 2]; 6] = [
    [1.0, 0.0],
    [0.5, 0.866_025_4],
    [-0.5, 0.866_025_4],
    [-1.0, 0.0],
    [-0.5, -0.866_025_4],
    [0.5, -0.866_025_4],
];

/// Append a closed tube between two rings of the given radii and heights.
fn tube(m: &mut MeshData, dirs: &[[f32; 2]], y0: f32, r0: f32, y1: f32, r1: f32) {
    let base = m.positions.len() as u32;
    let n = dirs.len() as u32;
    for (i, d) in dirs.iter().enumerate() {
        let u = i as f32 / n as f32;
        for (y, r) in [(y0, r0), (y1, r1)] {
            m.positions.push([d[0] * r, y, d[1] * r]);
            // Outward normal, tilted by the taper so a cone is not lit like a cylinder.
            let slope = (r0 - r1) / (y1 - y0).abs().max(1e-3);
            let n3 = normalize([d[0], slope, d[1]]);
            m.normals.push(n3);
            m.uvs.push([u, if y == y0 { 1.0 } else { 0.0 }]);
        }
    }
    for i in 0..n {
        let a = base + i * 2;
        let b = base + ((i + 1) % n) * 2;
        m.indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
    }
}

/// Append a cone: a ring at `y0` closing to a point at `y1`.
fn cone(m: &mut MeshData, dirs: &[[f32; 2]], y0: f32, r0: f32, y1: f32) {
    let base = m.positions.len() as u32;
    let n = dirs.len() as u32;
    let slope = r0 / (y1 - y0).abs().max(1e-3);
    for (i, d) in dirs.iter().enumerate() {
        m.positions.push([d[0] * r0, y0, d[1] * r0]);
        m.normals.push(normalize([d[0], slope, d[1]]));
        m.uvs.push([i as f32 / n as f32, 1.0]);
    }
    let apex = m.positions.len() as u32;
    m.positions.push([0.0, y1, 0.0]);
    m.normals.push([0.0, 1.0, 0.0]);
    m.uvs.push([0.5, 0.0]);
    for i in 0..n {
        m.indices
            .extend_from_slice(&[base + i, base + (i + 1) % n, apex]);
    }
}

/// Append a lobed blob — a sphere-ish canopy squashed and dented so it is not a billiard ball.
fn blob(m: &mut MeshData, cy: f32, rx: f32, ry: f32, lobes: f32) {
    // Two stacked rings plus caps: enough to read as a canopy, cheap enough to place thousands of.
    let base = m.positions.len() as u32;
    let rings = [(-0.55f32, 0.72f32), (0.0, 1.0), (0.55, 0.72)];
    for (k, (t, scale)) in rings.iter().enumerate() {
        for (i, d) in RING8.iter().enumerate() {
            // The lobe factor alternates around the ring, which is what dents the silhouette.
            let lobe = 1.0 + lobes * if i % 2 == 0 { 0.12 } else { -0.12 };
            let r = rx * scale * lobe;
            let y = cy + ry * t;
            m.positions.push([d[0] * r, y, d[1] * r]);
            m.normals.push(normalize([d[0], *t * 1.4, d[1]]));
            m.uvs
                .push([i as f32 / 8.0, k as f32 / (rings.len() - 1) as f32]);
        }
    }
    for k in 0..rings.len() as u32 - 1 {
        for i in 0..8u32 {
            let a = base + k * 8 + i;
            let b = base + k * 8 + (i + 1) % 8;
            m.indices.extend_from_slice(&[a, b, a + 8, a + 8, b, b + 8]);
        }
    }
    // Caps.
    for (t, sign) in [(-0.55f32, -1.0f32), (0.55, 1.0)] {
        let ring = if sign < 0.0 {
            0
        } else {
            rings.len() as u32 - 1
        };
        let apex = m.positions.len() as u32;
        m.positions.push([0.0, cy + ry * (t + sign * 0.45), 0.0]);
        m.normals.push([0.0, sign, 0.0]);
        m.uvs.push([0.5, 0.5]);
        for i in 0..8u32 {
            let a = base + ring * 8 + i;
            let b = base + ring * 8 + (i + 1) % 8;
            if sign < 0.0 {
                m.indices.extend_from_slice(&[a, apex, b]);
            } else {
                m.indices.extend_from_slice(&[a, b, apex]);
            }
        }
    }
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l2 = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if l2 <= 1e-20 {
        return [0.0, 1.0, 0.0];
    }
    let inv = 1.0 / l2.sqrt();
    [v[0] * inv, v[1] * inv, v[2] * inv]
}

/// Generate a stand-in at full detail.
#[must_use]
#[allow(clippy::too_many_lines)] // one match arm per stand-in; each is a short recipe, and splitting them
                                 // into seven functions would scatter a single readable table of what each plant is made of
pub fn build(kind: ProtoKind) -> ProtoMesh {
    let mut m = MeshData::default();
    let (stem, canopy, radius, height);
    let canopy_start;
    match kind {
        ProtoKind::BroadleafTree => {
            tube(&mut m, &RING6, 0.0, 0.30, 5.5, 0.18);
            canopy_start = m.positions.len() as u32;
            blob(&mut m, 8.0, 3.1, 2.6, 1.0);
            blob(&mut m, 10.4, 2.1, 1.6, 1.0);
            stem = [0.20, 0.14, 0.09];
            canopy = [0.17, 0.31, 0.12];
            radius = 3.1;
            height = 12.0;
        }
        ProtoKind::Conifer => {
            tube(&mut m, &RING6, 0.0, 0.26, 3.0, 0.16);
            canopy_start = m.positions.len() as u32;
            // Four tiers, each narrower and shorter — the classic conifer silhouette.
            for (i, (y, r, top)) in [
                (2.2f32, 2.6f32, 6.4f32),
                (5.0, 2.1, 9.2),
                (7.6, 1.6, 11.6),
                (9.8, 1.0, 13.6),
            ]
            .iter()
            .enumerate()
            {
                let _ = i;
                cone(&mut m, &RING8, *y, *r, *top);
            }
            stem = [0.19, 0.13, 0.09];
            canopy = [0.11, 0.24, 0.14];
            radius = 2.6;
            height = 13.6;
        }
        ProtoKind::Shrub => {
            tube(&mut m, &RING6, 0.0, 0.10, 0.35, 0.07);
            canopy_start = m.positions.len() as u32;
            blob(&mut m, 0.75, 0.75, 0.5, 1.4);
            stem = [0.22, 0.16, 0.10];
            canopy = [0.20, 0.30, 0.14];
            radius = 0.8;
            height = 1.3;
        }
        ProtoKind::GrassTuft => {
            canopy_start = 0;
            // Six blades leaning outwards: two triangles each, and nothing else. At the distance grass is
            // drawn this is indistinguishable from anything more expensive.
            for (i, d) in RING6.iter().enumerate() {
                let lean = 0.16 + (i % 3) as f32 * 0.05;
                let h = 0.30 + (i % 2) as f32 * 0.12;
                let base = m.positions.len() as u32;
                let w = 0.035;
                let (px, pz) = (-d[1] * w, d[0] * w);
                m.positions.push([px, 0.0, pz]);
                m.positions.push([-px, 0.0, -pz]);
                m.positions.push([d[0] * lean, h, d[1] * lean]);
                for _ in 0..3 {
                    m.normals.push(normalize([d[0] * 0.4, 1.0, d[1] * 0.4]));
                }
                m.uvs.push([0.0, 1.0]);
                m.uvs.push([1.0, 1.0]);
                m.uvs.push([0.5, 0.0]);
                m.indices.extend_from_slice(&[base, base + 1, base + 2]);
            }
            stem = [0.24, 0.34, 0.14];
            canopy = [0.24, 0.34, 0.14];
            radius = 0.2;
            height = 0.42;
        }
        ProtoKind::Boulder => {
            canopy_start = 0;
            // Centre at exactly the vertical radius so the blob's lower cap lands on y = 0 — a boulder that
            // dips below its own origin sinks into the terrain when scatter places it on the surface.
            blob(&mut m, 0.62, 0.9, 0.62, 1.8);
            stem = [0.36, 0.35, 0.33];
            canopy = [0.36, 0.35, 0.33];
            radius = 1.0;
            height = 1.24;
        }
        ProtoKind::DesertShrub => {
            canopy_start = 0;
            for (i, d) in RING8.iter().enumerate() {
                if i % 2 == 1 {
                    continue;
                }
                tube(&mut m, &RING6, 0.0, 0.05, 0.55, 0.02);
                // Lean each branch outwards by shifting the ring just added.
                let n = m.positions.len();
                for p in &mut m.positions[n - 12..] {
                    p[0] += d[0] * p[1] * 0.7;
                    p[2] += d[1] * p[1] * 0.7;
                }
            }
            stem = [0.34, 0.30, 0.18];
            canopy = [0.34, 0.30, 0.18];
            radius = 0.5;
            height = 0.7;
        }
        ProtoKind::Palm => {
            tube(&mut m, &RING6, 0.0, 0.22, 7.0, 0.15);
            canopy_start = m.positions.len() as u32;
            // Fronds: long thin triangles drooping from the crown.
            for d in &RING8 {
                let base = m.positions.len() as u32;
                let tip = [d[0] * 3.4, 6.4, d[1] * 3.4];
                m.positions.push([d[0] * 0.2, 7.0, d[1] * 0.2]);
                m.positions.push([-d[1] * 0.35, 7.0, d[0] * 0.35]);
                m.positions.push(tip);
                m.positions.push([d[1] * 0.35, 7.0, -d[0] * 0.35]);
                for _ in 0..4 {
                    m.normals.push([0.0, 1.0, 0.0]);
                }
                m.uvs.push([0.5, 1.0]);
                m.uvs.push([0.0, 1.0]);
                m.uvs.push([0.5, 0.0]);
                m.uvs.push([1.0, 1.0]);
                m.indices
                    .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }
            stem = [0.34, 0.26, 0.16];
            canopy = [0.19, 0.34, 0.15];
            radius = 3.4;
            height = 7.4;
        }
    }
    ProtoMesh {
        data: m,
        stem_color: stem,
        canopy_color: canopy,
        canopy_start,
        radius_m: radius,
        height_m: height,
    }
}

/// A coarser copy for a distance LOD: keep the silhouette, drop the detail.
///
/// Uses the same vertex-clustering idea the asset LOD generator does — quantize positions to a grid, weld,
/// drop collapsed triangles — because it is deterministic and dependency-free. `cell` is in metres.
#[must_use]
pub fn decimate(src: &MeshData, cell: f32) -> MeshData {
    // A LOD that collapses to nothing is worse than no LOD: the instance would silently vanish at that
    // distance. Sparse meshes (a palm's fronds, a grass tuft) collapse at cells a dense mesh survives, so
    // back off until something survives rather than trusting the caller's cell size.
    let mut cell = cell.max(1e-3);
    for _ in 0..5 {
        let out = decimate_at(src, cell);
        if !out.indices.is_empty() {
            return out;
        }
        cell *= 0.5;
    }
    src.clone()
}

fn decimate_at(src: &MeshData, cell: f32) -> MeshData {
    use std::collections::BTreeMap;
    let cell = cell.max(1e-3);
    let key = |p: [f32; 3]| {
        [
            (p[0] / cell).round() as i32,
            (p[1] / cell).round() as i32,
            (p[2] / cell).round() as i32,
        ]
    };
    // BTreeMap and first-encounter numbering: the output is bit-identical across runs and machines.
    let mut map: BTreeMap<[i32; 3], u32> = BTreeMap::new();
    let mut remap = vec![0u32; src.positions.len()];
    let mut out = MeshData::default();
    let mut accum: Vec<([f64; 3], [f64; 3], u32)> = Vec::new();
    for (i, p) in src.positions.iter().enumerate() {
        let k = key(*p);
        let idx = *map.entry(k).or_insert_with(|| {
            accum.push(([0.0; 3], [0.0; 3], 0));
            (accum.len() - 1) as u32
        });
        remap[i] = idx;
        let a = &mut accum[idx as usize];
        let n = src.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
        for c in 0..3 {
            a.0[c] += f64::from(p[c]);
            a.1[c] += f64::from(n[c]);
        }
        a.2 += 1;
    }
    for (sum, nsum, count) in &accum {
        let c = f64::from(*count).max(1.0);
        out.positions.push([
            (sum[0] / c) as f32,
            (sum[1] / c) as f32,
            (sum[2] / c) as f32,
        ]);
        out.normals.push(normalize([
            (nsum[0] / c) as f32,
            (nsum[1] / c) as f32,
            (nsum[2] / c) as f32,
        ]));
        out.uvs.push([0.5, 0.5]);
    }
    for t in src.indices.chunks_exact(3) {
        let (a, b, c) = (
            remap[t[0] as usize],
            remap[t[1] as usize],
            remap[t[2] as usize],
        );
        if a != b && b != c && a != c {
            out.indices.extend_from_slice(&[a, b, c]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ProtoKind; 7] = [
        ProtoKind::BroadleafTree,
        ProtoKind::Conifer,
        ProtoKind::Shrub,
        ProtoKind::GrassTuft,
        ProtoKind::Boulder,
        ProtoKind::DesertShrub,
        ProtoKind::Palm,
    ];

    #[test]
    fn every_stand_in_is_well_formed_and_stands_on_the_ground() {
        for kind in ALL {
            let p = build(kind);
            let m = &p.data;
            assert!(!m.indices.is_empty(), "{kind:?} has no geometry");
            assert_eq!(m.indices.len() % 3, 0);
            assert_eq!(m.positions.len(), m.normals.len());
            assert_eq!(m.positions.len(), m.uvs.len());
            for i in &m.indices {
                assert!(
                    (*i as usize) < m.positions.len(),
                    "{kind:?} index out of range"
                );
            }
            for n in &m.normals {
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!((len - 1.0).abs() < 1e-3, "{kind:?} normal not unit: {len}");
            }
            let lo = m.positions.iter().map(|q| q[1]).fold(f32::MAX, f32::min);
            let hi = m.positions.iter().map(|q| q[1]).fold(f32::MIN, f32::max);
            assert!(lo >= -1e-4, "{kind:?} sinks below its base: {lo}");
            assert!(hi > 0.2, "{kind:?} is flat: {hi}");
            // The declared height and radius must match the geometry, because scatter uses them to choose a
            // LOD by projected pixel size — a lie here shows up as foliage switching at the wrong distance.
            assert!(
                (hi - p.height_m).abs() < p.height_m * 0.35,
                "{kind:?} declares {} m but is {hi} m",
                p.height_m
            );
            let widest = m
                .positions
                .iter()
                .map(|q| (q[0] * q[0] + q[2] * q[2]).sqrt())
                .fold(0.0f32, f32::max);
            assert!(
                widest <= p.radius_m * 1.35 + 0.05,
                "{kind:?} declares {} m radius but reaches {widest} m",
                p.radius_m
            );
        }
    }

    #[test]
    fn stand_ins_are_bit_reproducible() {
        for kind in ALL {
            let a = build(kind);
            let b = build(kind);
            assert_eq!(a, b, "{kind:?} is not deterministic");
        }
    }

    #[test]
    fn decimation_keeps_the_silhouette_and_costs_less() {
        for kind in [
            ProtoKind::BroadleafTree,
            ProtoKind::Conifer,
            ProtoKind::Palm,
        ] {
            let full = build(kind);
            let coarse = decimate(&full.data, full.radius_m * 0.6);
            assert!(!coarse.indices.is_empty(), "{kind:?} decimated to nothing");
            assert!(
                coarse.tri_count() < full.data.tri_count(),
                "{kind:?} decimation did not reduce anything"
            );
            let hi_full = full
                .data
                .positions
                .iter()
                .map(|q| q[1])
                .fold(f32::MIN, f32::max);
            let hi_coarse = coarse
                .positions
                .iter()
                .map(|q| q[1])
                .fold(f32::MIN, f32::max);
            assert!(
                (hi_full - hi_coarse).abs() < full.height_m * 0.35,
                "{kind:?} lost its height: {hi_full} → {hi_coarse}"
            );
            // Deterministic too.
            assert_eq!(coarse, decimate(&full.data, full.radius_m * 0.6));
        }
    }

    #[test]
    fn ids_and_labels_are_distinct() {
        let mut ids: Vec<&str> = ALL.iter().map(|k| k.id()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "two prototypes share an id");
    }

    #[test]
    fn grass_is_cheap_enough_to_place_by_the_thousand() {
        // Ground clutter is placed at tens of thousands per hectare; if the stand-in is not tiny the whole
        // category is unusable and the preset would be lying about its density.
        let g = build(ProtoKind::GrassTuft);
        assert!(
            g.data.tri_count() <= 12,
            "grass tuft is {} triangles",
            g.data.tri_count()
        );
    }
}
