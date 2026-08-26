//! The presentation hall — the room an industrial visualisation is filmed in.
//!
//! # Why this module exists
//!
//! Three passes of camera work on the imported Skid Weld Line converged on the same finding: the
//! shortfall was not where the camera stood. The presentation ground was a single featureless quad
//! meeting a dark void, so *any* framing of *anything* had a mostly-empty frame — a 30 cm motor at
//! `extreme_wide` is a speck in an empty field, and the whole 262 m line broadside is a ribbon in an
//! empty field. Measured on the delivered films, the frames that failed the legibility floor were
//! overwhelmingly of that family, not of a buried lens. Composition cannot fix an empty room.
//!
//! So this module builds the room: a slab with bay joints and painted walkways, clad walls, a column
//! grid on the structural pitch, and a roof of trusses and purlins over it. It is **presentation, not
//! scene content** — the same standing as the ground quad it replaces. It creates no entities, enters
//! no document, changes no import report, and is invisible to every count the factory acceptance run
//! asserts. What it changes is what a camera standing in the scene can see.
//!
//! # Two properties the geometry is built for, not decorated with
//!
//! * **Every direction has something in it.** The planner's own vantage test asks, for nine directions
//!   spread across the frame, whether anything lies *beyond* the subject. In an empty scene the honest
//!   answer for most of them is "no", and the delivered films measured `backing` at 0.11 — one probe in
//!   nine — on every wide shot of the assembly. A closed shell answers all nine.
//! * **Detail at every distance.** Bay joints and the column grid read at a hundred metres as
//!   converging perspective, and at two metres as edges in frame. A flat plane reads as neither.
//!
//! # What it deliberately is not
//!
//! Not a light source, not a shadow caster, and not an occluder in the broad phase. It is lit by the
//! same environment as everything else and casts nothing, which is why adding a roof does not plunge
//! the machinery into darkness. A hall that dimmed the subject would be trading the thing being
//! presented for the room it is presented in.

use metrocalk_assets::MeshVertex;

/// Wall and roof-deck thickness, metres. Every enclosing surface is built as a pair of facing quads
/// this far apart, so the shell reads as built from something whichever side it is seen from — a
/// single-sided wall lit by an inward normal renders as a dark cut-out from outside.
const SHELL_THICKNESS: f32 = 0.25;

/// Height of the wall's darker lower band, metres. Industrial cladding is near-universally split this
/// way, and the line it draws is a horizon the eye can read scale against.
const DADO_HEIGHT: f32 = 2.4;

/// Height of one cladding band above the dado, metres. Alternating tones give the wall horizontal
/// structure without any extra geometry — the bands *are* the wall.
const CLADDING_BAND: f32 = 1.2;

/// Square section of a hall column, metres.
const COLUMN_SECTION: f32 = 0.45;

/// Painted line width, metres — the width a real floor marking is painted at.
const MARKING_WIDTH: f32 = 0.15;

/// Clear gap between the two lines of a walkway, metres.
const WALKWAY_GAP: f32 = 1.0;

/// How far a painted marking sits above the slab it is painted on, metres.
///
/// Bays no longer use this — they TILE with the grooves at a single height, so nothing overlaps and the
/// depth test is never asked to separate them (see `push_slab`). Markings genuinely do lie on top of a
/// bay, so they still need a lift, and 6 mm was not one: with a standard depth buffer and no reversed-Z
/// this viewport resolves centimetres at plant range, so the walkway lines broke into dashes at
/// distance. 2 cm is far below what reads as a step on a floor seen from standing height and far above
/// what the depth buffer can confuse.
const PAINT_LIFT: f32 = 0.02;

/// Bay-joint groove width, metres.
const JOINT_WIDTH: f32 = 0.06;

/// The largest number of bays the slab is divided into along one axis. A cap, not a target: without it
/// a hall sized to a kilometre of conveyor at a 6 m pitch would emit six-figure quad counts for detail
/// no camera can resolve.
const MAX_BAYS: usize = 72;

// ── Albedos, linear RGB ──────────────────────────────────────────────────────────────────────────
// Chosen against the delivered film rather than from a swatch book: the machinery is painted CAD in
// saturated yellows, blues and greys, and the room's job is to be the value that reads *behind* it.
// Every one of these is darker than the 0.30 grey it replaces, for the same reason that grey was
// itself darkened — a floor brighter than the subject standing on it leaves the subject nothing to be
// legible against.

/// Concrete slab.
const SLAB: [f32; 3] = [0.235, 0.232, 0.222];
/// The groove between bays.
const SLAB_JOINT: [f32; 3] = [0.125, 0.124, 0.120];
/// Safety yellow, as painted floor marking rather than as a warning label — desaturated from the CAD
/// yellows on purpose so a walkway is never mistaken for a machine.
const MARKING: [f32; 3] = [0.520, 0.380, 0.040];
/// Wall cladding, upper band.
const CLADDING_A: [f32; 3] = [0.335, 0.352, 0.375];
/// Wall cladding, alternate band.
const CLADDING_B: [f32; 3] = [0.285, 0.300, 0.322];
/// The wall's lower band.
const DADO: [f32; 3] = [0.105, 0.125, 0.155];
/// Wall and roof seen from outside — unlit industrial cladding, flatter and greyer than the interior.
const SHELL_OUTSIDE: [f32; 3] = [0.200, 0.210, 0.225];
/// Painted structural steel.
const STEEL: [f32; 3] = [0.190, 0.215, 0.255];
/// Roof purlins, one value lighter than the trusses so the two layers separate overhead.
const PURLIN: [f32; 3] = [0.235, 0.250, 0.275];
/// A roof-light panel between trusses. The brightest surface in the room by design: it is what makes
/// the space read as lit from above rather than as a lid.
const ROOFLIGHT: [f32; 3] = [0.720, 0.745, 0.790];
/// The roof deck over each truss line — the dark half of the ceiling's rhythm, and the reason a gap
/// overhead reads as a built surface rather than as sky-that-is-not-there.
const ROOF_DECK: [f32; 3] = [0.105, 0.112, 0.125];

/// A finished mesh for the presentation set: one vertex list, one index list, drawn as a unit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HallMesh {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
}

impl HallMesh {
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// The hall that stands around a given scene, in metres.
///
/// Every dimension is derived from the machinery, so the same code produces a 400 m weld-line hall and
/// a 12 m robot cell without a switch anywhere. `centre` is on the slab, at `floor_y`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hall {
    /// Slab centre. `[1]` is the slab's own height.
    pub centre: [f32; 3],
    /// Interior half-extent to the wall line, X.
    pub half_x: f32,
    /// Interior half-extent to the wall line, Z.
    pub half_z: f32,
    /// Clear height, slab to the underside of the roof structure.
    pub height: f32,
    /// Structural pitch — column spacing, truss spacing, and the slab's bay size.
    pub bay: f32,
    /// Half-extent of the machinery the hall was sized around, X. Drives where walkways are painted.
    pub machine_half_x: f32,
    /// Half-extent of the machinery the hall was sized around, Z.
    pub machine_half_z: f32,
}

/// The narrowest hall worth building, metres. Below this a "room" reads as a box the subject is jammed
/// into, which is worse than the open studio it replaces.
const MIN_HALF_EXTENT: f32 = 9.0;
/// Clear height bounds, metres. The floor is a working headroom; the ceiling stops a tall subject from
/// producing a cathedral whose roof is out of every frame.
const MIN_HEIGHT: f32 = 7.5;
const MAX_HEIGHT: f32 = 26.0;
/// The largest hall this will build, half-extent in metres. Past this the depth precision the viewport
/// has is the binding constraint, not the architecture.
const MAX_HALF_EXTENT: f32 = 1_200.0;

impl Hall {
    /// Size a hall around a scene's world bounds, or decline.
    ///
    /// Returns `None` for a scene with no finite bounds — an empty project, or one caught mid-import.
    /// Declining is the whole error path on purpose: a hall built around a degenerate bound would put
    /// non-representable vertices into the stream, and there is nothing to present anyway.
    #[must_use]
    pub fn around(lo: [f32; 3], hi: [f32; 3], floor_y: f32) -> Option<Self> {
        if !lo.iter().chain(hi.iter()).all(|v| v.is_finite()) || !floor_y.is_finite() {
            return None;
        }
        let machine_half_x = ((hi[0] - lo[0]) * 0.5).abs();
        let machine_half_z = ((hi[2] - lo[2]) * 0.5).abs();
        let machine_height = (hi[1] - lo[1]).abs();
        if !machine_half_x.is_finite() || !machine_half_z.is_finite() {
            return None;
        }

        // Aisle margin: the clear floor between the machinery and the wall. Proportional to the plant
        // so a long line gets a long hall, but bounded at both ends — a fixed margin puts a 30 cm
        // bracket in a broom cupboard, and an unbounded one puts the weld line in an aircraft hangar
        // whose far wall is a hundred metres past anything worth seeing.
        // PER AXIS, because a margin is a property of the side it is on. Driving both from the longer
        // span gave this plant - 269 m long and 32 m wide - a hall 82 m across: two and a half times the
        // width of the line it houses, so a shot down the line spent most of its frame width on empty
        // floor and the machinery it was pointed at stayed a strip. A long thin plant wants a long thin
        // building, which is also what actually gets built.
        let margin = |half: f32| (half * 0.30).clamp(6.0, 30.0);
        let half_x =
            (machine_half_x + margin(machine_half_x)).clamp(MIN_HALF_EXTENT, MAX_HALF_EXTENT);
        let half_z =
            (machine_half_z + margin(machine_half_z)).clamp(MIN_HALF_EXTENT, MAX_HALF_EXTENT);

        // Clear height: enough air over the tallest thing in the plant that the roof is structure
        // rather than a lid pressing on it.
        let height = (machine_height * 1.9 + 4.0).clamp(MIN_HEIGHT, MAX_HEIGHT);

        // Structural pitch. Solved from the hall's long side so the bay count lands in the range a real
        // portal frame uses, then held to buildable spacings.
        let long_side = (half_x.max(half_z)) * 2.0;
        let bay = (long_side / 18.0).clamp(6.0, 18.0);

        Some(Self {
            centre: [(lo[0] + hi[0]) * 0.5, floor_y, (lo[2] + hi[2]) * 0.5],
            half_x,
            half_z,
            height,
            bay,
            machine_half_x,
            machine_half_z,
        })
    }

    /// How far the slab runs past the wall line — an apron, so a camera that ends up outside is
    /// standing on something rather than over a void.
    #[must_use]
    pub fn apron(&self) -> f32 {
        (self.half_x.max(self.half_z) * 0.35).clamp(8.0, 120.0)
    }

    /// The interior box, as `(lo, hi)` in world space: slab to roof underside, wall line to wall line.
    #[must_use]
    pub fn interior(&self) -> ([f32; 3], [f32; 3]) {
        (
            [
                self.centre[0] - self.half_x,
                self.centre[1],
                self.centre[2] - self.half_z,
            ],
            [
                self.centre[0] + self.half_x,
                self.centre[1] + self.height,
                self.centre[2] + self.half_z,
            ],
        )
    }

    /// Is this point inside the room?
    // Not dead: the tests below are its only callers, because the shot solver consumes
    // `camera_room` directly (see that method's doc) rather than going through a wrapper. The
    // attribute is conditional rather than a bare `allow` so that a genuinely unreachable method
    // is still reported by the test build, where every one of these is exercised.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn contains(&self, p: [f64; 3]) -> bool {
        let (lo, hi) = self.interior();
        (0..3).all(|axis| p[axis] >= f64::from(lo[axis]) && p[axis] <= f64::from(hi[axis]))
    }

    /// The box a camera may stand in — the interior, inset by the clearance a lens needs off each
    /// surface.
    ///
    /// This is what makes a *closed* hall coherent with the direction. A wide card on a 262 m assembly
    /// solves to a stand-off of hundreds of metres — outside any building — and a camera there films
    /// the outside of a shed. Confined to this box, the same card becomes a deep view along the hall
    /// with the line receding into it: the subject occupies less of the frame and the frame occupies
    /// more of the plant, which is the trade a real factory establishing shot makes.
    ///
    /// The clearance is larger than the planner's own "is the camera buried" probe can reach, so
    /// confining a camera can never manufacture the defect it exists to avoid.
    #[must_use]
    pub fn camera_room(self) -> ([f32; 3], [f32; 3]) {
        let margin = (self.bay * 0.25).clamp(1.0, 4.0);
        let (lo, hi) = self.interior();
        let mut low = [0.0_f32; 3];
        let mut high = [0.0_f32; 3];
        for axis in 0..3 {
            // The floor clearance is a person's standing eye height rather than the wall clearance: a
            // camera on the slab is a legitimate industrial vantage, a camera in it is not.
            let inset = if axis == 1 { 1.6 } else { margin };
            let (mut a, mut b) = (lo[axis] + inset, hi[axis] - margin);
            if a > b {
                let mid = (lo[axis] + hi[axis]) * 0.5;
                a = mid;
                b = mid;
            }
            low[axis] = a;
            high[axis] = b;
        }
        (low, high)
    }

    /// How far a camera at `eye` must be able to see for the whole room to be inside its far plane.
    ///
    /// An upper bound on the distance from `eye` to any point of the built hall — slab and apron, wall
    /// line, roof — rather than a tight fit, because the cost of a slightly generous far plane is a
    /// little depth precision and the cost of a slightly tight one is a wall sliced out of the picture.
    ///
    /// # Why the cutscene needs to ask
    ///
    /// `cinematic_clip_planes` fits the far plane to the SUBJECT (`distance + radius * 4`). That is
    /// right when the subject is all there is to see, and wrong the moment the scene has a room around
    /// it: on a close shot of a one-metre mechanism the far plane lands a few metres out, and walls
    /// eighteen metres away are clipped away entirely. Measured on the first film shot in this hall,
    /// close shots showed a flat void behind the machinery — the exact background the room was built to
    /// remove — and wide shots showed the room with a hard diagonal edge across it where the far plane
    /// cut the wall.
    #[must_use]
    pub fn far_reach_from(&self, eye: [f32; 3]) -> f32 {
        let apron = self.apron();
        let dx = (eye[0] - self.centre[0]).abs() + self.half_x + apron;
        let dz = (eye[2] - self.centre[2]).abs() + self.half_z + apron;
        // Roof structure and the outer skin both sit above the clear height.
        let dy = (eye[1] - self.centre[1]).abs() + self.height + SHELL_THICKNESS * 4.0;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    /// Pull a camera position into [`Self::camera_room`]. Kept as its own name because the tests read
    /// better for it; the solver uses the box directly.
    // Not dead: the tests below are its only callers, because the shot solver consumes
    // `camera_room` directly (see that method's doc) rather than going through a wrapper. The
    // attribute is conditional rather than a bare `allow` so that a genuinely unreachable method
    // is still reported by the test build, where every one of these is exercised.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn clamp_inside(&self, eye: [f32; 3]) -> [f32; 3] {
        let (lo, hi) = self.camera_room();
        let mut out = eye;
        for axis in 0..3 {
            out[axis] = out[axis].clamp(lo[axis], hi[axis]);
        }
        out
    }

    /// Does a ray from `origin` along `dir` meet the hall shell within `max_t`?
    ///
    /// Answers the planner's backdrop question for the room itself. The shell is not in the scene BVH —
    /// it is not an entity — so without this the planner would keep reporting the void it used to be
    /// aimed at. `dir` need not be normalised; `max_t` is in units of `dir`'s length, matching the
    /// convention the scene's own ray tests use.
    #[must_use]
    pub fn shell_within(&self, origin: [f64; 3], dir: [f64; 3], max_t: f64) -> bool {
        let (lo, hi) = self.interior();
        let mut t_near = f64::NEG_INFINITY;
        let mut t_far = f64::INFINITY;
        for axis in 0..3 {
            let (low, high) = (f64::from(lo[axis]), f64::from(hi[axis]));
            if dir[axis].abs() < 1.0e-12 {
                // Parallel to this slab: a miss only if the origin is already outside it.
                if origin[axis] < low || origin[axis] > high {
                    return false;
                }
                continue;
            }
            let mut t0 = (low - origin[axis]) / dir[axis];
            let mut t1 = (high - origin[axis]) / dir[axis];
            if t0 > t1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            t_near = t_near.max(t0);
            t_far = t_far.min(t1);
            if t_near > t_far {
                return false;
            }
        }
        if t_far < 0.0 {
            return false; // the whole room is behind the ray
        }
        // Inside the room, the exit is the hit; outside it, the entry is.
        let hit = if t_near < 0.0 { t_far } else { t_near };
        hit.is_finite() && hit <= max_t
    }

    /// Build the room.
    #[must_use]
    pub fn build(&self) -> HallMesh {
        let mut mesh = HallMesh::default();
        self.push_slab(&mut mesh);
        self.push_markings(&mut mesh);
        self.push_walls(&mut mesh);
        self.push_columns(&mut mesh);
        self.push_roof(&mut mesh);
        mesh
    }

    // ── slab ─────────────────────────────────────────────────────────────────────────────────────

    fn push_slab(&self, mesh: &mut HallMesh) {
        let (cx, y, cz) = (self.centre[0], self.centre[1], self.centre[2]);
        let apron = self.apron();
        let (rx, rz) = (self.half_x + apron, self.half_z + apron);

        let (nx, nz) = self.bay_counts();
        let step_x = (rx * 2.0) / nx as f32;
        let step_z = (rz * 2.0) / nz as f32;
        let inset = JOINT_WIDTH * 0.5;

        // Bay edges as ONE function, so the grooves and the bays that meet at an edge are computed from
        // a bit-identical expression and abut without a seam. Two ways of arriving at "the same" float
        // is how a hairline of background appears between two surfaces that touch on paper.
        let edge_x = |i: usize| cx - rx + step_x * i as f32;
        let edge_z = |i: usize| cz - rz + step_z * i as f32;

        // THE GROOVES, as strips that TILE with the bays rather than as one dark quad lying underneath
        // them.
        //
        // The underneath version was a real defect, not a theoretical one. It put two coplanar surfaces
        // 3 mm apart and left the depth test to separate them; this viewport has no reversed-Z, uses a
        // standard `perspective_rh` and — at the editor camera's 0.1 m near plane and a 500 m stand-off —
        // resolves roughly 15 cm of depth, fifty times the separation. The winner flipped per pixel and
        // the apron rendered as blocky grey wedges across four hundred metres.
        //
        // Surfaces that never overlap cannot z-fight at ANY precision. That is a different kind of answer
        // from a bigger lift, which only moves the distance at which the same defect reappears.
        //
        // The strips do cross each other at the bay corners, and that overlap is harmless BY
        // CONSTRUCTION: both carry the same colour, so a depth fight between them is invisible.
        for ix in 0..=nx {
            let x = edge_x(ix);
            push_quad(
                mesh,
                [
                    [x - inset, y, cz - rz],
                    [x + inset, y, cz - rz],
                    [x + inset, y, cz + rz],
                    [x - inset, y, cz + rz],
                ],
                [0.0, 1.0, 0.0],
                SLAB_JOINT,
                0.0,
                0.92,
            );
        }
        for iz in 0..=nz {
            let z = edge_z(iz);
            push_quad(
                mesh,
                [
                    [cx - rx, y, z - inset],
                    [cx + rx, y, z - inset],
                    [cx + rx, y, z + inset],
                    [cx - rx, y, z + inset],
                ],
                [0.0, 1.0, 0.0],
                SLAB_JOINT,
                0.0,
                0.92,
            );
        }

        for ix in 0..nx {
            for iz in 0..nz {
                let (x0, x1) = (edge_x(ix) + inset, edge_x(ix + 1) - inset);
                let (z0, z1) = (edge_z(iz) + inset, edge_z(iz + 1) - inset);
                // Cast concrete is poured bay by bay and no two bays are the same tone. A fixed hash of
                // the bay index rather than a random: the room must be identical on every replay, or a
                // film cannot be compared with the one before it.
                let shade = 0.90 + 0.20 * hash01(ix as u32, iz as u32);
                push_quad(
                    mesh,
                    [[x0, y, z0], [x1, y, z0], [x1, y, z1], [x0, y, z1]],
                    [0.0, 1.0, 0.0],
                    [SLAB[0] * shade, SLAB[1] * shade, SLAB[2] * shade],
                    0.0,
                    0.92,
                );
            }
        }
    }

    /// How many bays the slab is divided into, capped so a very long hall does not emit detail no
    /// camera resolves.
    fn bay_counts(&self) -> (usize, usize) {
        let apron = self.apron();
        let span_x = (self.half_x + apron) * 2.0;
        let span_z = (self.half_z + apron) * 2.0;
        let count = |span: f32| ((span / self.bay).ceil() as usize).clamp(2, MAX_BAYS);
        (count(span_x), count(span_z))
    }

    // ── painted markings ─────────────────────────────────────────────────────────────────────────

    /// Two walkways, one either side of the machinery, painted as the pair of lines a real floor uses.
    ///
    /// They are placed off the *machinery's* footprint rather than off the hall's centre line, so on a
    /// plant that is not centred in its own building they still run where a person would actually walk.
    fn push_markings(&self, mesh: &mut HallMesh) {
        let (cx, y, cz) = (self.centre[0], self.centre[1], self.centre[2]);
        let along_x = self.half_x >= self.half_z;
        let y = y + PAINT_LIFT;

        // Run the full clear length, less a bay at each end — a walkway that touches the wall reads as
        // a mistake.
        let (run_half, cross_half, cross_centre) = if along_x {
            (
                (self.half_x - self.bay).max(self.bay),
                self.machine_half_z,
                cz,
            )
        } else {
            (
                (self.half_z - self.bay).max(self.bay),
                self.machine_half_x,
                cx,
            )
        };
        let stand_off =
            (cross_half + 2.2).min(if along_x { self.half_z } else { self.half_x } - 1.5);
        if !stand_off.is_finite() || stand_off <= 0.0 {
            return;
        }

        for side in [-1.0_f32, 1.0] {
            for line in [0.0_f32, WALKWAY_GAP + MARKING_WIDTH] {
                let offset = cross_centre + side * (stand_off + line);
                let (a, b) = (offset - MARKING_WIDTH * 0.5, offset + MARKING_WIDTH * 0.5);
                let corners = if along_x {
                    [
                        [cx - run_half, y, a],
                        [cx + run_half, y, a],
                        [cx + run_half, y, b],
                        [cx - run_half, y, b],
                    ]
                } else {
                    [
                        [a, y, cz - run_half],
                        [b, y, cz - run_half],
                        [b, y, cz + run_half],
                        [a, y, cz + run_half],
                    ]
                };
                push_quad(mesh, corners, [0.0, 1.0, 0.0], MARKING, 0.0, 0.75);
            }
        }
    }

    // ── walls ────────────────────────────────────────────────────────────────────────────────────

    fn push_walls(&self, mesh: &mut HallMesh) {
        let (cx, y, cz) = (self.centre[0], self.centre[1], self.centre[2]);
        let (hx, hz) = (self.half_x, self.half_z);
        let t = SHELL_THICKNESS;

        // Each wall as a pair: the face the room sees, and the face the world sees. `inward` is the
        // normal the interior face carries.
        let walls: [([f32; 3], [f32; 3], [f32; 3]); 4] = [
            // (inner-A, inner-B, inward normal) — A→B runs along the wall at slab level.
            (
                [cx - hx, y, cz - hz],
                [cx + hx, y, cz - hz],
                [0.0, 0.0, 1.0],
            ),
            (
                [cx + hx, y, cz + hz],
                [cx - hx, y, cz + hz],
                [0.0, 0.0, -1.0],
            ),
            (
                [cx - hx, y, cz + hz],
                [cx - hx, y, cz - hz],
                [1.0, 0.0, 0.0],
            ),
            (
                [cx + hx, y, cz - hz],
                [cx + hx, y, cz + hz],
                [-1.0, 0.0, 0.0],
            ),
        ];

        for (a, b, inward) in walls {
            let outward = [-inward[0], -inward[1], -inward[2]];
            let out = |p: [f32; 3]| [p[0] - inward[0] * t, p[1], p[2] - inward[2] * t];

            // Interior: a dark lower band, then alternating cladding to the roof.
            self.push_wall_face(mesh, a, b, inward, false);
            // Exterior: one flat tone. Nothing in this film films it, and a wall that looks the same
            // from both sides reads as a stage flat.
            push_wall_quad(
                mesh,
                out(a),
                out(b),
                (y, y + self.height + SHELL_THICKNESS),
                outward,
                SHELL_OUTSIDE,
                0.95,
            );
        }
    }

    /// One interior wall face: dado, then cladding bands to the roof.
    fn push_wall_face(
        &self,
        mesh: &mut HallMesh,
        a: [f32; 3],
        b: [f32; 3],
        normal: [f32; 3],
        _outer: bool,
    ) {
        let base = self.centre[1];
        let dado_top = base + DADO_HEIGHT.min(self.height);
        push_wall_quad(mesh, a, b, (base, dado_top), normal, DADO, 0.85);

        let mut lower = dado_top;
        let top = base + self.height;
        let mut band = 0usize;
        while lower < top - 1.0e-3 {
            let upper = (lower + CLADDING_BAND).min(top);
            let colour = if band.is_multiple_of(2) {
                CLADDING_A
            } else {
                CLADDING_B
            };
            push_wall_quad(mesh, a, b, (lower, upper), normal, colour, 0.80);
            lower = upper;
            band += 1;
            if band > 64 {
                break; // the height clamp makes this unreachable; it is here so it stays unreachable
            }
        }
    }

    // ── columns ──────────────────────────────────────────────────────────────────────────────────

    /// A column grid on the structural pitch, standing just inside each long wall.
    ///
    /// The single most useful thing in the room for a moving camera: a receding colonnade is depth,
    /// scale and parallax at once, and it is what turns a dolly along the line from a pan across a
    /// texture into a move through a space.
    fn push_columns(&self, mesh: &mut HallMesh) {
        let (cx, y, cz) = (self.centre[0], self.centre[1], self.centre[2]);
        let along_x = self.half_x >= self.half_z;
        let (run_half, stand) = if along_x {
            (self.half_x, self.half_z)
        } else {
            (self.half_z, self.half_x)
        };
        let inset = COLUMN_SECTION * 0.5 + 0.35;
        let count = ((run_half * 2.0 / self.bay).floor() as usize).clamp(2, MAX_BAYS);
        let step = (run_half * 2.0) / count as f32;
        let half = COLUMN_SECTION * 0.5;
        let top = y + self.height;

        for i in 0..=count {
            let along = -run_half + step * i as f32;
            for side in [-1.0_f32, 1.0] {
                let cross = side * (stand - inset);
                let (px, pz) = if along_x {
                    (cx + along, cz + cross)
                } else {
                    (cx + cross, cz + along)
                };
                push_box_sides(mesh, [px, y, pz], [px, top, pz], half, STEEL, 0.55);
            }
        }
    }

    // ── roof ─────────────────────────────────────────────────────────────────────────────────────

    /// Trusses across the short span at the structural pitch, purlins along the long axis, and a
    /// roof-light panel in every bay between them.
    ///
    /// Deliberately NOT a solid deck. A closed lid would be the one surface that reliably fills a
    /// frame with a single flat tone, and the upward-looking shots are exactly the ones the film was
    /// losing to void. Structure overhead reads as a building AND has edges in it.
    fn push_roof(&self, mesh: &mut HallMesh) {
        let (cx, y, cz) = (self.centre[0], self.centre[1], self.centre[2]);
        let along_x = self.half_x >= self.half_z;
        let (run_half, span_half) = if along_x {
            (self.half_x, self.half_z)
        } else {
            (self.half_z, self.half_x)
        };
        let top = y + self.height;
        let truss_depth = (self.height * 0.09).clamp(0.5, 1.3);
        let truss_half = (truss_depth * 0.35).clamp(0.18, 0.45);
        let count = ((run_half * 2.0 / self.bay).floor() as usize).clamp(2, MAX_BAYS);
        let step = (run_half * 2.0) / count as f32;

        for i in 0..=count {
            let along = -run_half + step * i as f32;
            let (a, b) = if along_x {
                (
                    [cx + along, top - truss_depth * 0.5, cz - span_half],
                    [cx + along, top - truss_depth * 0.5, cz + span_half],
                )
            } else {
                (
                    [cx - span_half, top - truss_depth * 0.5, cz + along],
                    [cx + span_half, top - truss_depth * 0.5, cz + along],
                )
            };
            push_box_sides(mesh, a, b, truss_half, STEEL, 0.55);

            // The roof plane, in two materials that TILE: a lit panel across each bay and a dark deck
            // strip over each truss line. Together they close the plane completely.
            //
            // The panels alone left the gaps open, and an open gap overhead is not "structure with
            // edges in it" - it is the void, which is the exact thing an upward-looking shot was
            // losing to. Measured on the first interior capture ever taken of this room, the ceiling
            // read as a near-black lid with one bright patch. Deck strips give the gaps a VALUE, and a
            // bright panel against a dark deck is more edge contrast than a bright panel against
            // nothing, not less.
            //
            // Tiled at ONE height for the same reason the slab is: overlapping coplanar surfaces are a
            // depth-test coin toss at this scene's scale.
            let panel_y = top + 0.05;
            let inset = step * 0.18;

            // Each strip of roof is a PAIR of faces, the same idiom the walls use: an outer skin seen
            // from the sky and an inner skin seen from the floor, a shell thickness apart. One quad
            // cannot serve both, and trying made the roof wrong at one end or the other — shaded
            // downward it was a black lid over the room (measured: a near-black ceiling under a
            // 0.72-albedo panel, because an overhead key gives N.L <= 0 and the only ambient available
            // to a downward normal comes from the dark lower hemisphere); flipped upward it read
            // correctly inside and turned the building into a blown-out black-and-white zebra seen
            // from above.
            //
            // BOTH faces are shaded as if facing the sky. For the outer skin that is simply true. For
            // the inner skin it is the one honest approximation available: a roof light is a
            // translucent panel with the sky behind it, so the light reaching an observer below IS the
            // light falling on its upper face, and this vertex format carries no emissive channel to
            // say so any other way. The deck strips borrow the same convention so the ceiling reads as
            // a dark surface rather than as an absence.
            let roof_strip =
                |mesh: &mut HallMesh, from: f32, to: f32, inner: [f32; 3], rough: f32| {
                    for (y, albedo) in
                        [(panel_y + SHELL_THICKNESS, SHELL_OUTSIDE), (panel_y, inner)]
                    {
                        let corners = if along_x {
                            [
                                [cx + from, y, cz - span_half],
                                [cx + to, y, cz - span_half],
                                [cx + to, y, cz + span_half],
                                [cx + from, y, cz + span_half],
                            ]
                        } else {
                            [
                                [cx - span_half, y, cz + from],
                                [cx - span_half, y, cz + to],
                                [cx + span_half, y, cz + to],
                                [cx + span_half, y, cz + from],
                            ]
                        };
                        push_quad(mesh, corners, [0.0, 1.0, 0.0], albedo, 0.0, rough);
                    }
                };

            // Deck over the truss line, both sides of it.
            roof_strip(
                mesh,
                (along - inset).max(-run_half),
                (along + inset).min(run_half),
                ROOF_DECK,
                0.8,
            );

            if i < count {
                roof_strip(mesh, along + inset, along + step - inset, ROOFLIGHT, 0.65);
            }
        }

        // Purlins the other way, tying the frames together.
        let purlin_half = (truss_half * 0.5).max(0.12);
        let runs = 5usize;
        for i in 0..=runs {
            let cross = -span_half + (span_half * 2.0 / runs as f32) * i as f32;
            let (a, b) = if along_x {
                (
                    [cx - run_half, top - truss_depth - purlin_half, cz + cross],
                    [cx + run_half, top - truss_depth - purlin_half, cz + cross],
                )
            } else {
                (
                    [cx + cross, top - truss_depth - purlin_half, cz - run_half],
                    [cx + cross, top - truss_depth - purlin_half, cz + run_half],
                )
            };
            push_box_sides(mesh, a, b, purlin_half, PURLIN, 0.6);
        }
    }
}

// ── geometry helpers ─────────────────────────────────────────────────────────────────────────────

fn vertex(p: [f32; 3], n: [f32; 3], c: [f32; 3], metallic: f32, roughness: f32) -> MeshVertex {
    MeshVertex {
        position: p,
        normal: n,
        color: c,
        metallic,
        roughness,
        uv: [0.0, 0.0], // untextured — binds the renderer's white dummy, so the baked colour is the colour
        tangent: [0.0, 0.0, 0.0, 1.0],
    }
}

/// One quad, wound `p[0] → p[1] → p[2] → p[3]`. The mesh pipeline does not cull, so the winding is a
/// convention rather than a visibility rule; the normal is what decides how it is lit.
fn push_quad(
    mesh: &mut HallMesh,
    p: [[f32; 3]; 4],
    n: [f32; 3],
    c: [f32; 3],
    metallic: f32,
    roughness: f32,
) {
    let base = mesh.vertices.len() as u32;
    for corner in p {
        mesh.vertices
            .push(vertex(corner, n, c, metallic, roughness));
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// A vertical wall panel between two ground-level points, from `low` to `high`.
fn push_wall_quad(
    mesh: &mut HallMesh,
    a: [f32; 3],
    b: [f32; 3],
    // The vertical span as one value rather than two loose floats: every caller already thinks of it as
    // one band, and a `low` that could be passed where a `high` belongs is a defect this shape removes.
    span: (f32, f32),
    n: [f32; 3],
    c: [f32; 3],
    roughness: f32,
) {
    let (low, high) = span;
    if high <= low {
        return;
    }
    push_quad(
        mesh,
        [
            [a[0], low, a[2]],
            [b[0], low, b[2]],
            [b[0], high, b[2]],
            [a[0], high, a[2]],
        ],
        n,
        c,
        0.0,
        roughness,
    );
}

/// The four long faces of a square-section member running from `a` to `b`. Ends are omitted: a column
/// meets the slab and the roof, and a truss meets the walls, so no end is ever visible.
fn push_box_sides(
    mesh: &mut HallMesh,
    a: [f32; 3],
    b: [f32; 3],
    half: f32,
    c: [f32; 3],
    roughness: f32,
) {
    let axis = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let length = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if !length.is_finite() || length <= 1.0e-4 || !half.is_finite() || half <= 0.0 {
        return;
    }
    let dir = [axis[0] / length, axis[1] / length, axis[2] / length];
    // Any two directions perpendicular to the member. World up unless the member IS vertical, in which
    // case a column would otherwise collapse its own cross-section to a line.
    let reference = if dir[1].abs() > 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = normalise(cross(dir, reference));
    let v = normalise(cross(dir, u));
    if u == [0.0; 3] || v == [0.0; 3] {
        return;
    }

    let offset = |s: f32, t: f32| {
        [
            u[0] * s * half + v[0] * t * half,
            u[1] * s * half + v[1] * t * half,
            u[2] * s * half + v[2] * t * half,
        ]
    };
    let add = |p: [f32; 3], o: [f32; 3]| [p[0] + o[0], p[1] + o[1], p[2] + o[2]];

    for (s, t, normal) in [
        (1.0_f32, 0.0_f32, u),
        (-1.0, 0.0, [-u[0], -u[1], -u[2]]),
        (0.0, 1.0, v),
        (0.0, -1.0, [-v[0], -v[1], -v[2]]),
    ] {
        // The face is the strip between the two in-plane corners at this side.
        let (e0, e1) = if s.abs() > 0.5 {
            (offset(s, -1.0), offset(s, 1.0))
        } else {
            (offset(-1.0, t), offset(1.0, t))
        };
        push_quad(
            mesh,
            [add(a, e0), add(b, e0), add(b, e1), add(a, e1)],
            normal,
            c,
            0.0,
            roughness,
        );
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalise(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len.is_finite() && len > 1.0e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0; 3]
    }
}

/// A stable `[0,1)` from two bay indices. Deterministic by construction — the room has to be identical
/// on every replay or two films of it cannot be compared.
fn hash01(a: u32, b: u32) -> f32 {
    let mut h: u32 = 2_166_136_261;
    for byte in a.to_le_bytes().iter().chain(b.to_le_bytes().iter()) {
        h ^= u32::from(*byte);
        h = h.wrapping_mul(16_777_619);
    }
    #[allow(clippy::cast_precision_loss)] // 24 bits into an f32 mantissa is exact
    {
        (h >> 8) as f32 / f32::from(u16::MAX) / 256.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weld_line() -> Hall {
        // The production Skid Weld Line's own proportions: a long, low, narrow plant.
        Hall::around([-131.0, 0.0, -3.5], [131.0, 4.2, 3.5], -0.02)
            .expect("finite bounds size a hall")
    }

    #[test]
    fn a_hall_is_sized_from_the_plant_and_stays_buildable_at_both_extremes() {
        let long = weld_line();
        assert!(
            long.half_x > 131.0,
            "the hall must clear the plant: {long:?}"
        );
        assert!(
            (7.5..=26.0).contains(&long.height),
            "clear height left the buildable range: {long:?}"
        );
        assert!(
            (6.0..=18.0).contains(&long.bay),
            "structural pitch left the buildable range: {long:?}"
        );

        // A single small part, framed on its own, must not be given a broom cupboard.
        let tiny = Hall::around([-0.15, 0.0, -0.15], [0.15, 0.3, 0.15], -0.02).expect("sized");
        assert!(
            tiny.half_x >= MIN_HALF_EXTENT && tiny.height >= MIN_HEIGHT,
            "a small subject got an unusable room: {tiny:?}"
        );
    }

    #[test]
    fn a_degenerate_bound_declines_rather_than_emitting_unrepresentable_vertices() {
        assert!(Hall::around([f32::NAN, 0.0, 0.0], [1.0, 1.0, 1.0], -0.02).is_none());
        assert!(Hall::around([0.0; 3], [f32::INFINITY, 1.0, 1.0], -0.02).is_none());
        assert!(Hall::around([0.0; 3], [1.0, 1.0, 1.0], f32::NAN).is_none());
    }

    #[test]
    fn every_vertex_of_a_built_hall_is_finite_and_the_index_stream_is_in_range() {
        for hall in [
            weld_line(),
            Hall::around([-0.15, 0.0, -0.15], [0.15, 0.3, 0.15], -0.02).expect("sized"),
            Hall::around([-600.0, -3.0, -280.0], [600.0, 30.0, 280.0], 0.0).expect("sized"),
        ] {
            let mesh = hall.build();
            assert!(
                mesh.triangle_count() > 200,
                "a room needs geometry: {hall:?}"
            );
            assert_eq!(mesh.indices.len() % 3, 0);
            for v in &mesh.vertices {
                assert!(
                    v.position.iter().all(|c| c.is_finite())
                        && v.normal.iter().all(|c| c.is_finite())
                        && v.color.iter().all(|c| c.is_finite()),
                    "a non-finite vertex would take the device down: {v:?} in {hall:?}"
                );
                let n = v.normal;
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!(
                    (len - 1.0).abs() < 1.0e-3,
                    "an unnormalised normal shades wrong: {n:?} (len {len}) in {hall:?}"
                );
            }
            let limit = mesh.vertices.len() as u32;
            assert!(
                mesh.indices.iter().all(|i| *i < limit),
                "an out-of-range index is a GPU fault, not a visual defect: {hall:?}"
            );
        }
    }

    #[test]
    fn the_room_is_identical_on_every_build() {
        // A film compared against the film before it needs the room to be the same room.
        let hall = weld_line();
        assert_eq!(hall.build(), hall.build());
    }

    #[test]
    fn a_backdrop_ray_from_inside_always_meets_the_shell_and_one_from_outside_can_miss() {
        let hall = weld_line();
        let inside = [0.0, 2.0, 0.0];
        for dir in [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.3, 0.8, -0.5],
        ] {
            assert!(
                hall.shell_within(inside, dir, 5_000.0),
                "a camera in the room must find a backdrop along {dir:?} — this is the whole point"
            );
        }
        // Reach matters: the same ray with nowhere near enough of it is honestly a miss.
        assert!(!hall.shell_within(inside, [0.0, 1.0, 0.0], 0.5));
        // Outside, pointing away.
        let outside = [0.0, 2.0, f64::from(hall.half_z) + 50.0];
        assert!(!hall.shell_within(outside, [0.0, 0.0, 1.0], 5_000.0));
        assert!(hall.shell_within(outside, [0.0, 0.0, -1.0], 5_000.0));
    }

    #[test]
    fn a_camera_solved_outside_the_building_is_brought_in_without_being_put_in_a_wall() {
        let hall = weld_line();
        // A wide card on a 262 m assembly solves to a stand-off no building contains.
        let solved = [0.0, 90.0, -600.0];
        let clamped = hall.clamp_inside(solved);
        assert!(
            hall.contains([
                f64::from(clamped[0]),
                f64::from(clamped[1]),
                f64::from(clamped[2])
            ]),
            "clamping must land inside the room: {clamped:?} in {hall:?}"
        );
        let (lo, hi) = hall.interior();
        for axis in 0..3 {
            assert!(
                clamped[axis] - lo[axis] > 0.9 && hi[axis] - clamped[axis] > 0.9,
                "clamping put the lens against a surface on axis {axis}: {clamped:?} in {hall:?}"
            );
        }
        // A camera already inside is left exactly where the direction put it.
        let interior = [10.0, 3.0, 4.0];
        assert_eq!(hall.clamp_inside(interior), interior);
    }

    #[test]
    fn the_slab_reaches_past_the_walls_so_an_exterior_vantage_still_stands_on_something() {
        let hall = weld_line();
        let mesh = hall.build();
        let furthest = mesh
            .vertices
            .iter()
            .filter(|v| (v.position[1] - hall.centre[1]).abs() < 0.05)
            .map(|v| (v.position[0] - hall.centre[0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            furthest > hall.half_x + 1.0,
            "the slab stops at the wall line ({furthest} vs {}), so anything outside floats",
            hall.half_x
        );
    }
}
