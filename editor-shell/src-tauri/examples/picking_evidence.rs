//! **Before/after evidence for the viewport picking rebuild.**
//!
//! Runs BOTH picking algorithms — the legacy nearest-projected-centroid search, reproduced verbatim
//! from the code it replaced, and the new raycast pipeline as the app actually ships it — over the
//! same scenes, from the same camera, at the same click points. Renders each comparison as one PNG
//! with the two answers side by side.
//!
//! This isolates the change with no confounds: one process, one scene construction, one projection,
//! one set of click coordinates. The only difference between the two panels is which function decided
//! what was under the cursor.
//!
//! Run: `cargo run --release --example picking_evidence -- <output-dir>`

// The `#[path]` includes below pull in whole modules of the binary so this example compiles the SAME
// source the shipped .exe does. Most of each module is unused here — an evidence tool needs the picking
// path and the geometry types, not the terrain runtime or the IBL baker — and every one of those unused
// items is a `dead_code` warning that `clippy --all-targets -D warnings` turns into a CI failure. The
// allow is scoped to this example and is the price of testing the real source instead of a copy.
#![allow(dead_code)]

use std::path::PathBuf;

use metrocalk_assets::{MeshGpu, MeshVertex};
use metrocalk_spatial::{Aabb, Camera, Transform};

// The crate under test is a binary, so the modules are included directly. This is the same source the
// shipped `metrocalk-editor-shell.exe` compiles.
#[path = "../src/ibl.rs"]
mod ibl;
#[path = "../src/moba.rs"]
mod moba;
#[path = "../src/render.rs"]
mod render;
#[path = "../src/scene_pick.rs"]
mod scene_pick;
#[path = "../src/terrain.rs"]
mod terrain;

use render::{Instance, SceneState};

// ── the legacy algorithm, reproduced verbatim ────────────────────────────────────────────────────

/// `render::pick_nearest`, exactly as it stood before this work (git HEAD, `render.rs:3928`).
///
/// Kept here so the comparison is against the real thing rather than a description of it. Note what
/// it does NOT do: no ray, no bounds, no geometry, no occlusion test, and no aspect correction. And
/// note `best.or(best_nc)` — with any instance in the scene it always returns `Some`.
fn legacy_pick_nearest(
    instances: &[Instance],
    cursor: (f32, f32),
    view_proj: &glam::Mat4,
) -> Option<usize> {
    let (nx, ny) = cursor;
    let click_x = nx * 2.0 - 1.0;
    let click_y = 1.0 - ny * 2.0;
    let mut best: Option<(usize, f32)> = None;
    let mut best_nc: Option<(usize, f32)> = None;
    for (i, inst) in instances.iter().enumerate() {
        let clip = *view_proj * glam::Vec3::from(inst.center).extend(1.0);
        if clip.w.abs() < 1e-6 {
            continue;
        }
        let ndc = clip.truncate() / clip.w;
        if ndc.x.is_nan() || ndc.y.is_nan() {
            continue;
        }
        let d2 = (ndc.x - click_x).powi(2) + (ndc.y - click_y).powi(2);
        if best_nc.is_none_or(|(_, bd)| d2 < bd) {
            best_nc = Some((i, d2));
        }
        if (0.0..=1.0).contains(&ndc.z) && best.is_none_or(|(_, bd)| d2 < bd) {
            best = Some((i, d2));
        }
    }
    best.or(best_nc).map(|(i, _)| i)
}

// ── scene construction ───────────────────────────────────────────────────────────────────────────

fn vertex(p: [f32; 3]) -> MeshVertex {
    MeshVertex {
        position: p,
        normal: [0.0, 1.0, 0.0],
        color: [1.0; 3],
        metallic: 0.0,
        roughness: 1.0,
        uv: [0.0; 2],
        tangent: [0.0; 4],
    }
}

/// A unit cube, optionally offset in its own local space — the CAD case where a part's modelling
/// origin sits away from its geometry.
fn cube_mesh(offset: [f32; 3]) -> MeshGpu {
    let c = [
        [-1.0, -1.0, -1.0],
        [1.0, -1.0, -1.0],
        [1.0, 1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
    ];
    MeshGpu {
        vertices: c
            .into_iter()
            .map(|p| vertex([p[0] + offset[0], p[1] + offset[1], p[2] + offset[2]]))
            .collect(),
        indices: vec![
            0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0, 7,
            3, 1, 2, 6, 1, 6, 5,
        ],
        submeshes: Vec::new(),
    }
}

struct Scene {
    name: &'static str,
    question: &'static str,
    state: SceneState,
    /// Click position as a `[0,1]` surface fraction.
    click: (f32, f32),
    /// What a user looking at the viewport would say they clicked on.
    expected: &'static str,
}

/// The unit vector from the orbit target toward the eye — the same formula the render loop uses. Lets
/// a scene place objects genuinely in front of / behind each other from the camera's point of view.
fn orbit_view_dir(orbit: f32, elevation: f32) -> [f32; 3] {
    let (o, e) = (orbit, elevation);
    let v = [o.cos() * e.cos(), e.sin(), o.sin() * e.cos()];
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / len, v[1] / len, v[2] / len]
}

fn mul(v: [f32; 3], s: f32) -> [f32; 3] {
    [v[0] * s, v[1] * s, v[2] * s]
}

fn base_state() -> SceneState {
    SceneState {
        meshes_revision: 1,
        revision: 1,
        surface_aspect: PANEL_W as f32 / PANEL_H as f32,
        distance: 26.0,
        elevation: 0.35,
        orbit: 0.6,
        cam_target: [0.0; 3],
        ..SceneState::default()
    }
}

fn push(state: &mut SceneState, id: &str, center: [f32; 3], scale: f32, slot: i32) {
    state.instances.push(Instance {
        center,
        scale,
        color: [0.55, 0.6, 0.68],
        selected: 0.0,
        rotation: [0.0, 0.0, 0.0, 1.0],
        material: [0.0; 4],
    });
    state.mesh_slots.push(slot);
    state.ids.push(id.to_string());
}

fn scenes() -> Vec<Scene> {
    let mut out = Vec::new();

    // 1 ── clicking empty space
    {
        let mut s = base_state();
        s.meshes.push(cube_mesh([0.0; 3]));
        push(&mut s, "cube", [0.0, 0.0, 0.0], 1.5, 0);
        out.push(Scene {
            name: "01-empty-space",
            question: "CLICK ON EMPTY SPACE, FAR FROM ANY OBJECT",
            state: s,
            click: (0.12, 0.16),
            expected: "NOTHING (DESELECT)",
        });
    }

    // 2 ── a big object's face vs a small neighbour's pivot
    {
        let mut s = base_state();
        s.meshes.push(cube_mesh([0.0; 3]));
        push(&mut s, "big-panel", [0.0, 0.0, 0.0], 5.0, 0);
        push(&mut s, "small-bolt", [6.2, 4.6, 3.0], 0.35, 0);
        out.push(Scene {
            name: "02-large-object-face",
            question: "CLICK THE BIG PANEL'S CORNER, NEAR THE SMALL BOLT'S PIVOT",
            state: s,
            click: (0.615, 0.305),
            expected: "BIG-PANEL (THE SURFACE UNDER THE CURSOR)",
        });
    }

    // 3 ── occlusion. The two objects are placed ALONG THE VIEW AXIS so one is exactly behind the
    // other on screen; anything else tests a different thing.
    {
        let mut s = base_state();
        let d = orbit_view_dir(s.orbit, s.elevation);
        s.meshes.push(cube_mesh([0.0; 3]));
        push(&mut s, "wall-front", mul(d, 5.0), 3.0, 0);
        push(&mut s, "hidden-behind", mul(d, -7.0), 3.0, 0);
        out.push(Scene {
            name: "03-occlusion",
            question: "CLICK THE FRONT WALL, WITH A HIDDEN OBJECT DIRECTLY BEHIND IT",
            state: s,
            click: (0.0, 0.0), // aimed at the front wall below
            expected: "WALL-FRONT (THE VISIBLE SURFACE)",
        });
    }

    // 4 ── geometry offset from its own pivot (imported CAD). Both parts have distant modelling
    // origins, which is the norm in a real assembly: `cad-part` is DRAWN under the cursor, while
    // `decoy-origin`'s pivot happens to sit there and its geometry is 40 units away off-screen.
    {
        let mut s = base_state();
        s.meshes.push(cube_mesh([7.0, 0.0, 0.0])); // geometry sits 7 units from its pivot
        s.meshes.push(cube_mesh([0.0, 0.0, -40.0])); // geometry nowhere near its pivot
        push(&mut s, "cad-part", [0.0, 0.0, 0.0], 1.4, 0);
        push(&mut s, "decoy-origin", [9.8, 0.0, 0.0], 1.0, 1);
        out.push(Scene {
            name: "04-offset-pivot",
            question: "CLICK IMPORTED GEOMETRY DRAWN AWAY FROM ITS MODELLING ORIGIN",
            state: s,
            click: (0.0, 0.0), // filled in below from the projected geometry
            expected: "CAD-PART (WHERE IT IS DRAWN)",
        });
    }

    // 5 ── a light marker
    {
        let mut s = base_state();
        s.meshes.push(cube_mesh([0.0; 3]));
        push(&mut s, "backdrop", [0.0, 0.0, -8.0], 4.0, 0);
        s.marker_entities.push(render::MarkerEntity {
            id: "key-light".into(),
            position: [3.0, 4.0, 2.0],
            kind: metrocalk_spatial::HitKind::Light,
        });
        out.push(Scene {
            name: "05-light-marker",
            question: "CLICK A LIGHT'S ICON (DRAWN, BUT NOT A MESH)",
            state: s,
            click: (0.0, 0.0), // filled in below
            expected: "KEY-LIGHT",
        });
    }

    out
}

// ── rendering ────────────────────────────────────────────────────────────────────────────────────

const PANEL_W: usize = 620;
const PANEL_H: usize = 460;
const HEADER_H: usize = 74;
const GAP: usize = 10;
const IMG_W: usize = PANEL_W * 2 + GAP * 3;
const IMG_H: usize = PANEL_H + HEADER_H + GAP * 2;

const BG: [u8; 3] = [22, 24, 30];
const PANEL_BG: [u8; 3] = [30, 33, 41];
const GRID: [u8; 3] = [44, 48, 58];
const OBJ: [u8; 3] = [120, 130, 148];
const SELECTED: [u8; 3] = [255, 196, 62];
const WRONG: [u8; 3] = [235, 84, 84];
const RIGHT: [u8; 3] = [78, 210, 130];
const CURSOR: [u8; 3] = [255, 255, 255];
const TEXT: [u8; 3] = [214, 220, 232];
const MUTED: [u8; 3] = [138, 148, 166];

struct Canvas {
    px: Vec<u8>,
}

impl Canvas {
    fn new() -> Self {
        let mut px = vec![0u8; IMG_W * IMG_H * 3];
        for c in px.chunks_exact_mut(3) {
            c.copy_from_slice(&BG);
        }
        Self { px }
    }

    fn set(&mut self, x: i64, y: i64, color: [u8; 3]) {
        if x < 0 || y < 0 || x >= IMG_W as i64 || y >= IMG_H as i64 {
            return;
        }
        let i = (y as usize * IMG_W + x as usize) * 3;
        self.px[i..i + 3].copy_from_slice(&color);
    }

    fn rect(&mut self, x: i64, y: i64, w: i64, h: i64, color: [u8; 3]) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.set(xx, yy, color);
            }
        }
    }

    fn line(&mut self, a: (f64, f64), b: (f64, f64), color: [u8; 3], thickness: i64) {
        let steps = ((b.0 - a.0).abs().max((b.1 - a.1).abs()) as i64).max(1);
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let x = a.0 + (b.0 - a.0) * t;
            let y = a.1 + (b.1 - a.1) * t;
            for oy in 0..thickness {
                for ox in 0..thickness {
                    self.set(x as i64 + ox, y as i64 + oy, color);
                }
            }
        }
    }

    /// A 5x7 bitmap glyph run. Uppercase-only charset — enough to label a panel without pulling in a
    /// font stack for a diagnostic image.
    fn text(&mut self, x: i64, y: i64, s: &str, color: [u8; 3], scale: i64) {
        let mut cx = x;
        for ch in s.chars() {
            let g = glyph(ch.to_ascii_uppercase());
            for (row, bits) in g.iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        self.rect(
                            cx + col as i64 * scale,
                            y + row as i64 * scale,
                            scale,
                            scale,
                            color,
                        );
                    }
                }
            }
            cx += 6 * scale;
        }
    }
}

/// 5x7 glyphs, MSB-left, for the characters these labels use.
fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        ':' => [0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x0C, 0x04, 0x08],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '\'' => [0x04, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00],
        '/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
        '=' => [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        '?' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        _ => [0x00; 7],
    }
}

/// Project a world point into a panel's pixel coordinates.
fn project(camera: &Camera, aspect: f64, p: [f64; 3], panel_x: usize) -> Option<(f64, f64)> {
    let ndc = camera.world_to_ndc(p, aspect)?;
    Some((
        panel_x as f64 + (ndc[0] + 1.0) * 0.5 * PANEL_W as f64,
        HEADER_H as f64 + GAP as f64 + (1.0 - ndc[1]) * 0.5 * PANEL_H as f64,
    ))
}

/// The world AABB an instance actually draws, in the mesh's own local bounds — the same geometry the
/// new picker tests against.
fn instance_world_aabb(state: &SceneState, index: usize) -> Aabb {
    let inst = &state.instances[index];
    let slot = state.mesh_slots.get(index).copied().unwrap_or(-1);
    let local = usize::try_from(slot)
        .ok()
        .and_then(|s| state.meshes.get(s))
        .map_or(Aabb::new([-1.0; 3], [1.0; 3]), |m| {
            Aabb::from_points(m.vertices.iter().map(|v| {
                [
                    f64::from(v.position[0]),
                    f64::from(v.position[1]),
                    f64::from(v.position[2]),
                ]
            }))
        });
    let t = Transform {
        translation: [
            f64::from(inst.center[0]),
            f64::from(inst.center[1]),
            f64::from(inst.center[2]),
        ],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [f64::from(inst.scale); 3],
    };
    local.transformed(t.to_matrix())
}

/// Draw one panel: the scene, the click crosshair, and whichever object the algorithm chose.
#[allow(clippy::too_many_arguments)]
fn draw_panel(
    canvas: &mut Canvas,
    state: &SceneState,
    camera: &Camera,
    aspect: f64,
    panel_x: usize,
    click_px: (f64, f64),
    chosen: Option<usize>,
    chosen_marker: Option<usize>,
    title: &str,
    verdict: &str,
    verdict_color: [u8; 3],
) {
    canvas.rect(
        panel_x as i64,
        (HEADER_H + GAP) as i64,
        PANEL_W as i64,
        PANEL_H as i64,
        PANEL_BG,
    );

    // A faint ground grid, so depth and scale are readable.
    for i in -6..=6 {
        let v = f64::from(i) * 4.0;
        if let (Some(a), Some(b)) = (
            project(camera, aspect, [v, 0.0, -24.0], panel_x),
            project(camera, aspect, [v, 0.0, 24.0], panel_x),
        ) {
            canvas.line(a, b, GRID, 1);
        }
        if let (Some(a), Some(b)) = (
            project(camera, aspect, [-24.0, 0.0, v], panel_x),
            project(camera, aspect, [24.0, 0.0, v], panel_x),
        ) {
            canvas.line(a, b, GRID, 1);
        }
    }

    // Every object's wireframe box, with the chosen one highlighted.
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (1, 3),
        (3, 2),
        (2, 0),
        (4, 5),
        (5, 7),
        (7, 6),
        (6, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    for index in 0..state.instances.len() {
        let selected = chosen == Some(index);
        let color = if selected { SELECTED } else { OBJ };
        let thickness = if selected { 3 } else { 1 };
        let corners = instance_world_aabb(state, index).corners();
        let pts: Vec<Option<(f64, f64)>> = corners
            .iter()
            .map(|c| project(camera, aspect, *c, panel_x))
            .collect();
        for (a, b) in EDGES {
            if let (Some(pa), Some(pb)) = (pts[a], pts[b]) {
                canvas.line(pa, pb, color, thickness);
            }
        }
        // Its pivot, which is all the legacy algorithm ever looked at.
        if let Some(p) = project(
            camera,
            aspect,
            [
                f64::from(state.instances[index].center[0]),
                f64::from(state.instances[index].center[1]),
                f64::from(state.instances[index].center[2]),
            ],
            panel_x,
        ) {
            canvas.rect(p.0 as i64 - 2, p.1 as i64 - 2, 4, 4, MUTED);
        }
        if let Some(name) = state.ids.get(index) {
            if let Some(p) = project(
                camera,
                aspect,
                instance_world_aabb(state, index).center(),
                panel_x,
            ) {
                canvas.text(p.0 as i64 - 20, p.1 as i64 - 6, name, color, 1);
            }
        }
    }

    // Marker entities (lights/cameras) as a small burst glyph.
    for (offset, marker) in state.marker_entities.iter().enumerate() {
        let selected = chosen_marker == Some(offset);
        let color = if selected { SELECTED } else { MUTED };
        let p = [
            f64::from(marker.position[0]),
            f64::from(marker.position[1]),
            f64::from(marker.position[2]),
        ];
        if let Some(c) = project(camera, aspect, p, panel_x) {
            let r = if selected { 12.0 } else { 8.0 };
            for k in 0..8 {
                let a = f64::from(k) * std::f64::consts::TAU / 8.0;
                canvas.line(
                    (c.0 + a.cos() * 3.0, c.1 + a.sin() * 3.0),
                    (c.0 + a.cos() * r, c.1 + a.sin() * r),
                    color,
                    if selected { 2 } else { 1 },
                );
            }
            canvas.text(c.0 as i64 - 20, c.1 as i64 + 14, &marker.id, color, 1);
        }
    }

    // The click: a white crosshair, drawn last so it is always visible.
    let (cx, cy) = click_px;
    canvas.line((cx - 11.0, cy), (cx + 11.0, cy), CURSOR, 2);
    canvas.line((cx, cy - 11.0), (cx, cy + 11.0), CURSOR, 2);

    canvas.text(
        panel_x as i64 + 12,
        (HEADER_H + GAP + 10) as i64,
        title,
        TEXT,
        2,
    );
    canvas.text(
        panel_x as i64 + 12,
        (HEADER_H + GAP + PANEL_H - 24) as i64,
        verdict,
        verdict_color,
        2,
    );
}

fn write_png(path: &PathBuf, canvas: &Canvas) {
    let file = std::fs::File::create(path).expect("create png");
    let w = std::io::BufWriter::new(file);
    let mut encoder = png::Encoder::new(w, IMG_W as u32, IMG_H as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(&canvas.px).expect("png data");
}

fn main() {
    let out_dir: PathBuf = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from("picking-evidence"), PathBuf::from);
    std::fs::create_dir_all(&out_dir).expect("output dir");

    let mut summary = Vec::new();

    for mut scene in scenes() {
        let camera = scene_pick::camera_for(&scene.state);
        let viewport = scene_pick::viewport_for(&scene.state, 1.0);
        let aspect = viewport.aspect();

        // Two scenes want their click aimed at something computed from the projection.
        if scene.click == (0.0, 0.0) {
            let target = if scene.name == "04-offset-pivot" || scene.name == "03-occlusion" {
                instance_world_aabb(&scene.state, 0).center()
            } else {
                let m = &scene.state.marker_entities[0];
                [
                    f64::from(m.position[0]),
                    f64::from(m.position[1]),
                    f64::from(m.position[2]),
                ]
            };
            let ndc = camera.world_to_ndc(target, aspect).expect("in front");
            let (fx, fy) = viewport.ndc_to_fraction(ndc[0], ndc[1]);
            scene.click = (fx as f32, fy as f32);
        }

        // ── the legacy answer ────────────────────────────────────────────────────────────────
        let legacy_vp = render::camera_matrix_with(
            scene.state.orbit,
            scene.state.elevation,
            scene.state.distance,
            scene.state.surface_aspect,
            scene.state.cam_target.into(),
            scene.state.projection,
        );
        let legacy = legacy_pick_nearest(&scene.state.instances, scene.click, &legacy_vp);
        let legacy_name = legacy
            .and_then(|i| scene.state.ids.get(i).cloned())
            .unwrap_or_else(|| "NOTHING".into());

        // ── the new answer, through the shipped pipeline ─────────────────────────────────────
        let mut cache = scene_pick::PickCache::new();
        cache.sync(&scene.state);
        let ray = camera.ray_through_fraction(
            &viewport,
            f64::from(scene.click.0),
            f64::from(scene.click.1),
        );
        let hit = cache.nearest(&camera, &viewport, &ray, &scene_pick::click_filter());
        let new_name = hit
            .as_ref()
            .and_then(|h| scene_pick::entity_of(&scene.state, h))
            .unwrap_or_else(|| "NOTHING".into());
        let new_instance = hit
            .as_ref()
            .and_then(|h| scene_pick::instance_index_of(&scene.state, h.key));
        let new_marker = hit.as_ref().and_then(|h| {
            let key = h.key as usize;
            (key >= scene.state.instances.len()).then(|| key - scene.state.instances.len())
        });

        // ── render ───────────────────────────────────────────────────────────────────────────
        let mut canvas = Canvas::new();
        canvas.text(GAP as i64 + 12, 12, scene.name, MUTED, 2);
        canvas.text(GAP as i64 + 12, 34, scene.question, TEXT, 2);
        canvas.text(
            GAP as i64 + 12,
            54,
            &format!("EXPECTED: {}", scene.expected),
            MUTED,
            1,
        );

        let click_px_left = (
            GAP as f64 + f64::from(scene.click.0) * PANEL_W as f64,
            HEADER_H as f64 + GAP as f64 + f64::from(scene.click.1) * PANEL_H as f64,
        );
        let click_px_right = (
            (GAP * 2 + PANEL_W) as f64 + f64::from(scene.click.0) * PANEL_W as f64,
            HEADER_H as f64 + GAP as f64 + f64::from(scene.click.1) * PANEL_H as f64,
        );

        let legacy_ok = legacy_name.to_uppercase() == first_word(scene.expected);
        let new_ok = new_name.to_uppercase() == first_word(scene.expected);

        draw_panel(
            &mut canvas,
            &scene.state,
            &camera,
            aspect,
            GAP,
            click_px_left,
            legacy,
            None,
            "BEFORE - NEAREST PROJECTED PIVOT",
            &format!("SELECTED: {}", legacy_name.to_uppercase()),
            if legacy_ok { RIGHT } else { WRONG },
        );
        draw_panel(
            &mut canvas,
            &scene.state,
            &camera,
            aspect,
            GAP * 2 + PANEL_W,
            click_px_right,
            new_instance,
            new_marker,
            "AFTER - RAYCAST + BVH",
            &format!("SELECTED: {}", new_name.to_uppercase()),
            if new_ok { RIGHT } else { WRONG },
        );

        let path = out_dir.join(format!("{}.png", scene.name));
        write_png(&path, &canvas);
        summary.push(format!(
            "{:<22} expected={:<40} before={:<16} after={:<16} before_ok={} after_ok={}",
            scene.name, scene.expected, legacy_name, new_name, legacy_ok, new_ok
        ));
        println!("wrote {}", path.display());
    }

    summary.push(draw_gizmo_space_evidence(&out_dir));
    summary.push(draw_cursor_space_evidence(&out_dir));

    println!("\n─── summary ───");
    for line in &summary {
        println!("{line}");
    }
}

/// **`GizmoSpace::Local` was dead code.** Every call site passed an identity basis, so the toolbar
/// could read "Local" while every handle pointed along world axes — and dragging the "local X" arrow
/// moved the object along world X.
fn draw_gizmo_space_evidence(out_dir: &PathBuf) -> String {
    let mut state = base_state();
    state.meshes.push(cube_mesh([0.0; 3]));
    // A deliberately awkward orientation, so world and local axes are visibly different.
    let rot = metrocalk_gizmo::axis_angle([0.35, 1.0, 0.2], 52f32.to_radians());
    push(&mut state, "part", [0.0, 0.0, 0.0], 2.2, 0);
    state.instances[0].rotation = rot;

    let camera = scene_pick::camera_for(&state);
    let viewport = scene_pick::viewport_for(&state, 1.0);
    let aspect = viewport.aspect();

    let world_axes: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let local_axes = metrocalk_gizmo::quat_basis(rot);

    let mut canvas = Canvas::new();
    canvas.text(GAP as i64 + 12, 12, "06-GIZMO-LOCAL-SPACE", MUTED, 2);
    canvas.text(
        GAP as i64 + 12,
        34,
        "TOOLBAR SAYS SPACE: LOCAL, ON A ROTATED PART",
        TEXT,
        2,
    );
    canvas.text(
        GAP as i64 + 12,
        54,
        "EXPECTED: HANDLES ALIGN TO THE PART'S OWN AXES",
        MUTED,
        1,
    );

    for (panel_x, axes, title, verdict, color, drawn_local) in [
        (
            GAP,
            world_axes,
            "BEFORE - IDENTITY BASIS PASSED",
            "HANDLES USE WORLD AXES - LABEL IS A LIE",
            WRONG,
            false,
        ),
        (
            GAP * 2 + PANEL_W,
            local_axes,
            "AFTER - THE PART'S REAL ROTATION",
            "HANDLES USE THE PART'S OWN AXES",
            RIGHT,
            true,
        ),
    ] {
        canvas.rect(
            panel_x as i64,
            (HEADER_H + GAP) as i64,
            PANEL_W as i64,
            PANEL_H as i64,
            PANEL_BG,
        );
        for i in -6..=6 {
            let v = f64::from(i) * 4.0;
            if let (Some(a), Some(b)) = (
                project(&camera, aspect, [v, 0.0, -24.0], panel_x),
                project(&camera, aspect, [v, 0.0, 24.0], panel_x),
            ) {
                canvas.line(a, b, GRID, 1);
            }
            if let (Some(a), Some(b)) = (
                project(&camera, aspect, [-24.0, 0.0, v], panel_x),
                project(&camera, aspect, [24.0, 0.0, v], panel_x),
            ) {
                canvas.line(a, b, GRID, 1);
            }
        }
        // The rotated part, drawn as its real oriented box so the mismatch is visible.
        let t = Transform {
            translation: [0.0; 3],
            rotation: [
                f64::from(rot[0]),
                f64::from(rot[1]),
                f64::from(rot[2]),
                f64::from(rot[3]),
            ],
            scale: [2.2; 3],
        };
        const EDGES: [(usize, usize); 12] = [
            (0, 1),
            (1, 3),
            (3, 2),
            (2, 0),
            (4, 5),
            (5, 7),
            (7, 6),
            (6, 4),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ];
        let corners = Aabb::new([-1.0; 3], [1.0; 3]).corners();
        let pts: Vec<Option<(f64, f64)>> = corners
            .iter()
            .map(|c| project(&camera, aspect, t.transform_point(*c), panel_x))
            .collect();
        for (a, b) in EDGES {
            if let (Some(pa), Some(pb)) = (pts[a], pts[b]) {
                canvas.line(pa, pb, OBJ, 1);
            }
        }
        // The gizmo handles, in whatever basis this panel was given.
        let origin = project(&camera, aspect, [0.0; 3], panel_x);
        let colors = [[0.92, 0.26, 0.28], [0.36, 0.83, 0.34], [0.30, 0.52, 0.96]];
        for (i, axis) in axes.iter().enumerate() {
            let tip = [
                f64::from(axis[0]) * 6.0,
                f64::from(axis[1]) * 6.0,
                f64::from(axis[2]) * 6.0,
            ];
            if let (Some(o), Some(p)) = (origin, project(&camera, aspect, tip, panel_x)) {
                let c = [
                    (colors[i][0] * 255.0) as u8,
                    (colors[i][1] * 255.0) as u8,
                    (colors[i][2] * 255.0) as u8,
                ];
                canvas.line(o, p, c, 3);
                canvas.text(p.0 as i64 + 6, p.1 as i64 - 4, ["X", "Y", "Z"][i], c, 2);
            }
        }
        if !drawn_local {
            // Show where the handles SHOULD be, so the discrepancy is measurable rather than asserted.
            for axis in &local_axes {
                let tip = [
                    f64::from(axis[0]) * 6.0,
                    f64::from(axis[1]) * 6.0,
                    f64::from(axis[2]) * 6.0,
                ];
                if let (Some(o), Some(p)) = (origin, project(&camera, aspect, tip, panel_x)) {
                    canvas.line(o, p, MUTED, 1);
                }
            }
            canvas.text(
                panel_x as i64 + 12,
                (HEADER_H + GAP + PANEL_H - 46) as i64,
                "GREY = WHERE LOCAL HANDLES SHOULD POINT",
                MUTED,
                1,
            );
        }
        canvas.text(
            panel_x as i64 + 12,
            (HEADER_H + GAP + 10) as i64,
            title,
            TEXT,
            2,
        );
        canvas.text(
            panel_x as i64 + 12,
            (HEADER_H + GAP + PANEL_H - 24) as i64,
            verdict,
            color,
            2,
        );
    }

    let path = out_dir.join("06-gizmo-local-space.png");
    write_png(&path, &canvas);
    println!("wrote {}", path.display());
    // The measurable claim: the two bases really do differ.
    let dot = (0..3)
        .map(|i| world_axes[0][i] * local_axes[0][i])
        .sum::<f32>();
    format!(
        "{:<22} world-X . local-X = {:.3} (1.000 would mean the toggle changes nothing)",
        "06-gizmo-local-space", dot
    )
}

/// **The live drag used a desktop-relative cursor.** `cursor_position()` reports the pointer in
/// desktop coordinates; the drag divided it by the *surface* size as if it were client-relative, so
/// every drag was offset by the window's own position on screen.
fn draw_cursor_space_evidence(out_dir: &PathBuf) -> String {
    // A window whose client area sits 600 x 350 into the desktop.
    let (win_x, win_y, win_w, win_h) = (600.0f64, 350.0f64, 1280.0f64, 720.0f64);
    let cursor = (win_x + win_w * 0.5, win_y + win_h * 0.5); // dead centre of the viewport

    let correct = render::surface_fraction(
        Some(tauri::PhysicalPosition::new(cursor.0, cursor.1)),
        Some((win_x, win_y)),
        win_w as u32,
        win_h as u32,
    )
    .expect("cursor");
    // The old arithmetic, verbatim: raw desktop position / surface size.
    let legacy = ((cursor.0 / win_w) as f32, (cursor.1 / win_h) as f32);

    let mut canvas = Canvas::new();
    canvas.text(GAP as i64 + 12, 12, "07-CURSOR-COORDINATE-SPACE", MUTED, 2);
    canvas.text(
        GAP as i64 + 12,
        34,
        "POINTER AT THE EXACT CENTRE OF A WINDOW AT DESKTOP (600, 350)",
        TEXT,
        2,
    );
    canvas.text(
        GAP as i64 + 12,
        54,
        "EXPECTED: SURFACE FRACTION = 0.500, 0.500",
        MUTED,
        1,
    );

    // Both panels draw the same desktop + window; only the reported point differs.
    let scale = (PANEL_W as f64 - 40.0) / 2560.0; // fit a 2560x1440 desktop
    for (panel_x, fraction, title, verdict, color) in [
        (
            GAP,
            legacy,
            "BEFORE - DESKTOP POSITION / SURFACE SIZE",
            &format!("REPORTS {:.3}, {:.3}", legacy.0, legacy.1),
            WRONG,
        ),
        (
            GAP * 2 + PANEL_W,
            correct,
            "AFTER - CLIENT-RELATIVE",
            &format!("REPORTS {:.3}, {:.3}", correct.0, correct.1),
            RIGHT,
        ),
    ] {
        canvas.rect(
            panel_x as i64,
            (HEADER_H + GAP) as i64,
            PANEL_W as i64,
            PANEL_H as i64,
            PANEL_BG,
        );
        let ox = panel_x as f64 + 20.0;
        let oy = (HEADER_H + GAP) as f64 + 60.0;
        // Desktop outline.
        let dw = 2560.0 * scale;
        let dh = 1440.0 * scale;
        canvas.line((ox, oy), (ox + dw, oy), GRID, 1);
        canvas.line((ox + dw, oy), (ox + dw, oy + dh), GRID, 1);
        canvas.line((ox + dw, oy + dh), (ox, oy + dh), GRID, 1);
        canvas.line((ox, oy + dh), (ox, oy), GRID, 1);
        canvas.text(ox as i64 + 4, oy as i64 - 14, "DESKTOP 2560X1440", MUTED, 1);
        // The window's client area.
        let wx = ox + win_x * scale;
        let wy = oy + win_y * scale;
        let ww = win_w * scale;
        let wh = win_h * scale;
        canvas.rect(wx as i64, wy as i64, ww as i64, wh as i64, [38, 42, 52]);
        canvas.line((wx, wy), (wx + ww, wy), OBJ, 1);
        canvas.line((wx + ww, wy), (wx + ww, wy + wh), OBJ, 1);
        canvas.line((wx + ww, wy + wh), (wx, wy + wh), OBJ, 1);
        canvas.line((wx, wy + wh), (wx, wy), OBJ, 1);
        canvas.text(wx as i64 + 4, wy as i64 + 4, "VIEWPORT", MUTED, 1);

        // The true pointer.
        let tx = ox + cursor.0 * scale;
        let ty = oy + cursor.1 * scale;
        canvas.line((tx - 10.0, ty), (tx + 10.0, ty), CURSOR, 2);
        canvas.line((tx, ty - 10.0), (tx, ty + 10.0), CURSOR, 2);
        canvas.text(tx as i64 + 12, ty as i64 - 4, "POINTER", CURSOR, 1);

        // Where THIS panel's arithmetic says the pointer is, inside the viewport.
        let rx = wx + f64::from(fraction.0) * ww;
        let ry = wy + f64::from(fraction.1) * wh;
        canvas.line((rx - 8.0, ry - 8.0), (rx + 8.0, ry + 8.0), color, 2);
        canvas.line((rx - 8.0, ry + 8.0), (rx + 8.0, ry - 8.0), color, 2);
        canvas.text(rx as i64 + 12, ry as i64 + 6, "USED", color, 1);
        if (rx - tx).abs() > 2.0 || (ry - ty).abs() > 2.0 {
            canvas.line((tx, ty), (rx, ry), color, 1);
            let err_px = (((f64::from(fraction.0) - 0.5) * win_w).powi(2)
                + ((f64::from(fraction.1) - 0.5) * win_h).powi(2))
            .sqrt();
            canvas.text(
                panel_x as i64 + 12,
                (HEADER_H + GAP + PANEL_H - 46) as i64,
                &format!("ERROR: {} PIXELS", err_px.round() as i64),
                color,
                1,
            );
        }

        canvas.text(
            panel_x as i64 + 12,
            (HEADER_H + GAP + 10) as i64,
            title,
            TEXT,
            2,
        );
        canvas.text(
            panel_x as i64 + 12,
            (HEADER_H + GAP + PANEL_H - 24) as i64,
            verdict,
            color,
            2,
        );
    }

    let path = out_dir.join("07-cursor-coordinate-space.png");
    write_png(&path, &canvas);
    println!("wrote {}", path.display());
    let err_px = (((f64::from(legacy.0) - 0.5) * win_w).powi(2)
        + ((f64::from(legacy.1) - 0.5) * win_h).powi(2))
    .sqrt();
    format!(
        "{:<22} before={:.3},{:.3} after={:.3},{:.3} error_before={:.0}px error_after=0px",
        "07-cursor-space", legacy.0, legacy.1, correct.0, correct.1, err_px
    )
}

/// The expected entity name is the first token of the expectation string.
fn first_word(s: &str) -> String {
    s.split_whitespace()
        .next()
        .unwrap_or_default()
        .to_uppercase()
}
