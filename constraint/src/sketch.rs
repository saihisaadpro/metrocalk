//! The plain-array 2D sketch model — points, circles, and the typed constraint palette. **No `ezpz`
//! type appears here** (invariant 5); this is what the whole workspace sees. The solver
//! ([`crate::Solver`]) turns a `Sketch` into a [`crate::SolveResult`]; `ezpz` is confined to
//! [`crate::ezpz_backend`].

use serde::{Deserialize, Serialize};
use std::fmt;

/// A 2D coordinate (millimetres). In a [`Sketch`] the point positions ARE the **witness** — the initial
/// configuration the solver starts from. Storing it in the op is the "flip"-fixing mechanism (ADR-076):
/// every peer re-solves from the same start, so a two-branch system lands on the same branch.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// X, in millimetres.
    pub x: f64,
    /// Y, in millimetres.
    pub y: f64,
}

impl Point {
    /// A point at `(x, y)`.
    #[must_use]
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Which coordinate of a point a scalar constraint pins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    /// The x coordinate.
    X,
    /// The y coordinate.
    Y,
}

/// A circle: its centre is a sketch point (by index); its radius is a solved scalar (initial guess).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CircleDef {
    /// Index of the centre point in [`Sketch::points`].
    pub center: usize,
    /// The radius (its value here is the initial guess; a [`ConstraintDef::CircleRadius`] pins it).
    pub radius: f64,
}

/// The typed 2D constraint palette — a **declared subset** (coincident/tangent/parallel/perpendicular/
/// dimension) plus DOF analysis. Points and circles are referenced BY INDEX into the [`Sketch`]. Splines,
/// patterns, driven equations, and full GD&T-grade sketch constraints are a named future; **3D assembly
/// mates are a named future** (D-Cubed-class), out of scope for M15.6.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ConstraintDef {
    /// Pin one coordinate of a point to a value.
    Fixed {
        /// The point index.
        point: usize,
        /// Which coordinate.
        axis: Axis,
        /// The pinned value (mm).
        value: f64,
    },
    /// Two points coincide.
    Coincident {
        /// First point.
        a: usize,
        /// Second point.
        b: usize,
    },
    /// Two points are a given (Euclidean) distance apart (a dimension).
    Distance {
        /// First point.
        a: usize,
        /// Second point.
        b: usize,
        /// The distance (mm).
        d: f64,
    },
    /// Two points are a given horizontal distance apart.
    HorizontalDistance {
        /// First point.
        a: usize,
        /// Second point.
        b: usize,
        /// The horizontal distance (mm).
        d: f64,
    },
    /// Two points are a given vertical distance apart.
    VerticalDistance {
        /// First point.
        a: usize,
        /// Second point.
        b: usize,
        /// The vertical distance (mm).
        d: f64,
    },
    /// The segment a→b is horizontal.
    Horizontal {
        /// Segment start.
        a: usize,
        /// Segment end.
        b: usize,
    },
    /// The segment a→b is vertical.
    Vertical {
        /// Segment start.
        a: usize,
        /// Segment end.
        b: usize,
    },
    /// The segments a0→a1 and b0→b1 are parallel.
    Parallel {
        /// Line A start.
        a0: usize,
        /// Line A end.
        a1: usize,
        /// Line B start.
        b0: usize,
        /// Line B end.
        b1: usize,
    },
    /// The segments a0→a1 and b0→b1 are perpendicular.
    Perpendicular {
        /// Line A start.
        a0: usize,
        /// Line A end.
        a1: usize,
        /// Line B start.
        b0: usize,
        /// Line B end.
        b1: usize,
    },
    /// The segments meet at the given angle (degrees).
    Angle {
        /// Line A start.
        a0: usize,
        /// Line A end.
        a1: usize,
        /// Line B start.
        b0: usize,
        /// Line B end.
        b1: usize,
        /// The angle between them (degrees).
        degrees: f64,
    },
    /// A circle has the given radius (a dimension).
    CircleRadius {
        /// The circle index.
        circle: usize,
        /// The radius (mm).
        r: f64,
    },
    /// The segment l0→l1 is tangent to the circle.
    LineTangentToCircle {
        /// Segment start.
        l0: usize,
        /// Segment end.
        l1: usize,
        /// The circle index.
        circle: usize,
    },
    /// The point p is a given perpendicular distance from the segment l0→l1.
    PointLineDistance {
        /// The point.
        p: usize,
        /// Segment start.
        l0: usize,
        /// Segment end.
        l1: usize,
        /// The distance (mm).
        d: f64,
    },
}

/// A malformed sketch — an out-of-range index or a degenerate segment. Returned (never panicked) so a
/// bad authoring input is **explained, not a crash** (the M10.2 gate, applied to constraints).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SketchError {
    /// A constraint references a point index that doesn't exist.
    PointOutOfRange {
        /// The offending constraint index.
        constraint: usize,
        /// The bad point index.
        point: usize,
        /// How many points the sketch has.
        points: usize,
    },
    /// A constraint references a circle index that doesn't exist.
    CircleOutOfRange {
        /// The offending constraint index.
        constraint: usize,
        /// The bad circle index.
        circle: usize,
        /// How many circles the sketch has.
        circles: usize,
    },
    /// A circle's centre references a point index that doesn't exist.
    CircleCenterOutOfRange {
        /// The offending circle index.
        circle: usize,
        /// The bad centre index.
        center: usize,
        /// How many points the sketch has.
        points: usize,
    },
    /// A line-segment constraint names the same point for both endpoints (no direction).
    DegenerateSegment {
        /// The offending constraint index.
        constraint: usize,
    },
    /// A dimension value is not finite / not sensible (NaN, ∞, or a negative distance/radius).
    BadValue {
        /// The offending constraint index.
        constraint: usize,
        /// What was wrong, in plain words.
        why: String,
    },
}

impl fmt::Display for SketchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PointOutOfRange {
                constraint,
                point,
                points,
            } => write!(
                f,
                "constraint {constraint} references point {point}, but the sketch has only {points} point(s)"
            ),
            Self::CircleOutOfRange {
                constraint,
                circle,
                circles,
            } => write!(
                f,
                "constraint {constraint} references circle {circle}, but the sketch has only {circles} circle(s)"
            ),
            Self::CircleCenterOutOfRange {
                circle,
                center,
                points,
            } => write!(
                f,
                "circle {circle}'s centre is point {center}, but the sketch has only {points} point(s)"
            ),
            Self::DegenerateSegment { constraint } => write!(
                f,
                "constraint {constraint} names a zero-length segment (both endpoints are the same point)"
            ),
            Self::BadValue { constraint, why } => {
                write!(f, "constraint {constraint} has a bad value: {why}")
            }
        }
    }
}

impl std::error::Error for SketchError {}

/// A 2D sketch: points (their positions = the **witness**), circles, and constraints. Constraints carry
/// no witness of their own — the witness is the point/radius initial configuration, stored once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Sketch {
    /// The points. Their positions are the initial guesses (the witness).
    pub points: Vec<Point>,
    /// The circles.
    pub circles: Vec<CircleDef>,
    /// The constraints. A constraint's INDEX is its stable id for the minimal-conflicting-set.
    pub constraints: Vec<ConstraintDef>,
}

impl Sketch {
    /// An empty sketch.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a point at its initial-guess position; returns its index.
    pub fn add_point(&mut self, x: f64, y: f64) -> usize {
        self.points.push(Point::new(x, y));
        self.points.len() - 1
    }

    /// Add a circle (centre = an existing point index, radius = an initial guess); returns its index.
    pub fn add_circle(&mut self, center: usize, radius: f64) -> usize {
        self.circles.push(CircleDef { center, radius });
        self.circles.len() - 1
    }

    /// Add a constraint; returns its index (its stable id for the minimal-conflicting-set).
    pub fn add(&mut self, c: ConstraintDef) -> usize {
        self.constraints.push(c);
        self.constraints.len() - 1
    }

    /// A plain-language label for constraint `i` (used by the minimal-conflicting-set explanation).
    ///
    /// # Panics
    /// If `i` is out of range.
    #[must_use]
    pub fn describe(&self, i: usize) -> String {
        describe_constraint(i, &self.constraints[i])
    }

    /// Validate that every index is in range and every segment is non-degenerate and every value sane.
    /// Returns the FIRST problem found — an authoring input is explained, never a panic downstream.
    ///
    /// # Errors
    /// A [`SketchError`] naming the offending constraint/circle in plain words.
    pub fn validate(&self) -> Result<(), SketchError> {
        let np = self.points.len();
        let nc = self.circles.len();
        for (ci, c) in self.circles.iter().enumerate() {
            if c.center >= np {
                return Err(SketchError::CircleCenterOutOfRange {
                    circle: ci,
                    center: c.center,
                    points: np,
                });
            }
        }
        for (i, con) in self.constraints.iter().enumerate() {
            check_constraint(i, con, np, nc)?;
        }
        Ok(())
    }
}

// A dimension value must be finite (NaN/∞ would poison the solve); distances/radii must be non-negative.
fn check_finite(constraint: usize, v: f64, what: &str) -> Result<(), SketchError> {
    if !v.is_finite() {
        return Err(SketchError::BadValue {
            constraint,
            why: format!("{what} is {v} (must be a finite number)"),
        });
    }
    Ok(())
}

fn check_nonneg(constraint: usize, v: f64, what: &str) -> Result<(), SketchError> {
    check_finite(constraint, v, what)?;
    if v < 0.0 {
        return Err(SketchError::BadValue {
            constraint,
            why: format!("{what} is {v} (must not be negative)"),
        });
    }
    Ok(())
}

fn pt(constraint: usize, p: usize, np: usize) -> Result<(), SketchError> {
    if p >= np {
        return Err(SketchError::PointOutOfRange {
            constraint,
            point: p,
            points: np,
        });
    }
    Ok(())
}

fn seg(constraint: usize, a: usize, b: usize, np: usize) -> Result<(), SketchError> {
    pt(constraint, a, np)?;
    pt(constraint, b, np)?;
    if a == b {
        return Err(SketchError::DegenerateSegment { constraint });
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // one exhaustive match arm per constraint kind — flat by design
fn check_constraint(
    i: usize,
    con: &ConstraintDef,
    np: usize,
    nc: usize,
) -> Result<(), SketchError> {
    match *con {
        ConstraintDef::Fixed { point, value, .. } => {
            pt(i, point, np)?;
            check_finite(i, value, "the pinned value")?;
        }
        ConstraintDef::Coincident { a, b } => {
            pt(i, a, np)?;
            pt(i, b, np)?;
        }
        ConstraintDef::Distance { a, b, d } => {
            pt(i, a, np)?;
            pt(i, b, np)?;
            check_nonneg(i, d, "the distance")?;
        }
        ConstraintDef::HorizontalDistance { a, b, d }
        | ConstraintDef::VerticalDistance { a, b, d } => {
            pt(i, a, np)?;
            pt(i, b, np)?;
            check_finite(i, d, "the distance")?;
        }
        ConstraintDef::Horizontal { a, b } | ConstraintDef::Vertical { a, b } => {
            seg(i, a, b, np)?;
        }
        ConstraintDef::Parallel { a0, a1, b0, b1 }
        | ConstraintDef::Perpendicular { a0, a1, b0, b1 } => {
            seg(i, a0, a1, np)?;
            seg(i, b0, b1, np)?;
        }
        ConstraintDef::Angle {
            a0,
            a1,
            b0,
            b1,
            degrees,
        } => {
            seg(i, a0, a1, np)?;
            seg(i, b0, b1, np)?;
            check_finite(i, degrees, "the angle")?;
        }
        ConstraintDef::CircleRadius { circle, r } => {
            if circle >= nc {
                return Err(SketchError::CircleOutOfRange {
                    constraint: i,
                    circle,
                    circles: nc,
                });
            }
            check_nonneg(i, r, "the radius")?;
        }
        ConstraintDef::LineTangentToCircle { l0, l1, circle } => {
            seg(i, l0, l1, np)?;
            if circle >= nc {
                return Err(SketchError::CircleOutOfRange {
                    constraint: i,
                    circle,
                    circles: nc,
                });
            }
        }
        ConstraintDef::PointLineDistance { p, l0, l1, d } => {
            pt(i, p, np)?;
            seg(i, l0, l1, np)?;
            check_nonneg(i, d, "the distance")?;
        }
    }
    Ok(())
}

/// The plain-language rendering of a constraint — the phrase that appears in a minimal-conflicting-set.
#[must_use]
pub fn describe_constraint(i: usize, c: &ConstraintDef) -> String {
    match *c {
        ConstraintDef::Fixed { point, axis, value } => {
            let a = match axis {
                Axis::X => "x",
                Axis::Y => "y",
            };
            format!("#{i} fix p{point}.{a} = {value}")
        }
        ConstraintDef::Coincident { a, b } => format!("#{i} p{a} coincides with p{b}"),
        ConstraintDef::Distance { a, b, d } => format!("#{i} distance(p{a}, p{b}) = {d}"),
        ConstraintDef::HorizontalDistance { a, b, d } => {
            format!("#{i} horizontal distance(p{a}, p{b}) = {d}")
        }
        ConstraintDef::VerticalDistance { a, b, d } => {
            format!("#{i} vertical distance(p{a}, p{b}) = {d}")
        }
        ConstraintDef::Horizontal { a, b } => {
            format!("#{i} segment p{a}\u{2192}p{b} is horizontal")
        }
        ConstraintDef::Vertical { a, b } => format!("#{i} segment p{a}\u{2192}p{b} is vertical"),
        ConstraintDef::Parallel { a0, a1, b0, b1 } => {
            format!("#{i} p{a0}\u{2192}p{a1} \u{2225} p{b0}\u{2192}p{b1}")
        }
        ConstraintDef::Perpendicular { a0, a1, b0, b1 } => {
            format!("#{i} p{a0}\u{2192}p{a1} \u{22a5} p{b0}\u{2192}p{b1}")
        }
        ConstraintDef::Angle {
            a0,
            a1,
            b0,
            b1,
            degrees,
        } => format!("#{i} angle(p{a0}\u{2192}p{a1}, p{b0}\u{2192}p{b1}) = {degrees}\u{b0}"),
        ConstraintDef::CircleRadius { circle, r } => format!("#{i} radius(c{circle}) = {r}"),
        ConstraintDef::LineTangentToCircle { l0, l1, circle } => {
            format!("#{i} segment p{l0}\u{2192}p{l1} tangent to c{circle}")
        }
        ConstraintDef::PointLineDistance { p, l0, l1, d } => {
            format!("#{i} distance(p{p}, line p{l0}\u{2192}p{l1}) = {d}")
        }
    }
}
