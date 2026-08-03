use std::error::Error;
use std::fmt::{self, Display, Formatter};

use crate::{MatchConfig, Vec2Mm};

/// Stable identity for one cooked, non-branching MOB-2 lane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LaneId(pub u16);

impl LaneId {
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Authoritative location along a lane centerline. World coordinates are a derived presentation value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LanePosition {
    pub lane: LaneId,
    pub progress_mm: u64,
}

/// Cooked one-route lane definition. Point order is semantic and therefore is never re-sorted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneSpec {
    pub id: LaneId,
    pub centerline: Vec<Vec2Mm>,
    pub half_width_mm: u32,
}

/// Validated lane with bounded cumulative distances for deterministic projection and movement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledLane {
    id: LaneId,
    centerline: Vec<Vec2Mm>,
    cumulative_mm: Vec<u64>,
    half_width_mm: u32,
}

impl CompiledLane {
    pub fn compile(spec: &LaneSpec, config: MatchConfig) -> Result<Self, LaneError> {
        if spec.centerline.len() < 2 {
            return Err(LaneError::TooFewPoints);
        }
        if spec.centerline.len() > usize::from(config.max_lane_points_per_lane) {
            return Err(LaneError::PointBudgetExceeded);
        }
        if spec.half_width_mm == 0 {
            return Err(LaneError::ZeroWidth);
        }
        if spec
            .centerline
            .iter()
            .any(|point| !config.map_bounds.contains(*point))
        {
            return Err(LaneError::PointOutOfBounds);
        }

        let mut cumulative_mm = Vec::with_capacity(spec.centerline.len());
        cumulative_mm.push(0);
        let mut total = 0_u64;
        for points in spec.centerline.windows(2) {
            let segment_squared = points[0].squared_distance(points[1]);
            if segment_squared == 0 {
                return Err(LaneError::DuplicateConsecutivePoint);
            }
            let segment = u64::try_from(integer_sqrt(segment_squared))
                .map_err(|_| LaneError::LengthOverflow)?;
            total = total
                .checked_add(segment)
                .ok_or(LaneError::LengthOverflow)?;
            cumulative_mm.push(total);
        }

        Ok(Self {
            id: spec.id,
            centerline: spec.centerline.clone(),
            cumulative_mm,
            half_width_mm: spec.half_width_mm,
        })
    }

    #[must_use]
    pub const fn id(&self) -> LaneId {
        self.id
    }

    #[must_use]
    pub fn length_mm(&self) -> u64 {
        self.cumulative_mm.last().copied().unwrap_or(0)
    }

    #[must_use]
    pub const fn half_width_mm(&self) -> u32 {
        self.half_width_mm
    }

    #[must_use]
    pub fn centerline(&self) -> &[Vec2Mm] {
        &self.centerline
    }

    /// Convert authoritative progress to a deterministic world point. Interpolation rounds to the nearest
    /// millimetre, with exact endpoints and checked wide intermediates.
    pub fn point_at(&self, position: LanePosition) -> Result<Vec2Mm, LaneError> {
        if position.lane != self.id {
            return Err(LaneError::WrongLane);
        }
        if position.progress_mm > self.length_mm() {
            return Err(LaneError::ProgressOutOfBounds);
        }
        if position.progress_mm == self.length_mm() {
            return self
                .centerline
                .last()
                .copied()
                .ok_or(LaneError::TooFewPoints);
        }

        let segment_index = self
            .cumulative_mm
            .partition_point(|progress| *progress <= position.progress_mm)
            .saturating_sub(1)
            .min(self.centerline.len() - 2);
        let segment_start = self.cumulative_mm[segment_index];
        let segment_end = self.cumulative_mm[segment_index + 1];
        let offset = position.progress_mm - segment_start;
        let length = segment_end - segment_start;
        Ok(interpolate(
            self.centerline[segment_index],
            self.centerline[segment_index + 1],
            offset,
            length,
        ))
    }

    /// Project a world destination onto the authored corridor. Equal-distance ties choose lower progress,
    /// then the lower segment, so the result is independent of iteration or insertion order.
    pub fn project(&self, point: Vec2Mm) -> Result<LanePosition, LaneError> {
        let mut best: Option<(u128, u64, usize)> = None;

        for (segment_index, points) in self.centerline.windows(2).enumerate() {
            let start = points[0];
            let end = points[1];
            let vx = i128::from(end.x) - i128::from(start.x);
            let vy = i128::from(end.y) - i128::from(start.y);
            let wx = i128::from(point.x) - i128::from(start.x);
            let wy = i128::from(point.y) - i128::from(start.y);
            let length_squared = vx * vx + vy * vy;
            let dot = wx * vx + wy * vy;
            let segment_length =
                self.cumulative_mm[segment_index + 1] - self.cumulative_mm[segment_index];
            let offset = if dot <= 0 {
                0
            } else if dot >= length_squared {
                segment_length
            } else {
                u64::try_from((dot * i128::from(segment_length)) / length_squared)
                    .map_err(|_| LaneError::LengthOverflow)?
            };
            let progress = self.cumulative_mm[segment_index] + offset;
            let projected = interpolate(start, end, offset, segment_length);
            let candidate = (point.squared_distance(projected), progress, segment_index);
            if best.is_none_or(|current| candidate < current) {
                best = Some(candidate);
            }
        }

        let (distance_squared, progress_mm, _) = best.ok_or(LaneError::TooFewPoints)?;
        if distance_squared > u128::from(self.half_width_mm).pow(2) {
            return Err(LaneError::OutsideCorridor);
        }
        Ok(LanePosition {
            lane: self.id,
            progress_mm,
        })
    }

    /// Move by an exact progress budget; unlike world-vector integration this cannot stall at bends.
    pub fn advance_towards(
        &self,
        current: LanePosition,
        target: LanePosition,
        max_step_mm: u32,
    ) -> Result<(LanePosition, bool), LaneError> {
        if current.lane != self.id || target.lane != self.id {
            return Err(LaneError::WrongLane);
        }
        if current.progress_mm > self.length_mm() || target.progress_mm > self.length_mm() {
            return Err(LaneError::ProgressOutOfBounds);
        }
        let step = u64::from(max_step_mm);
        let next = if current.progress_mm < target.progress_mm {
            current
                .progress_mm
                .saturating_add(step)
                .min(target.progress_mm)
        } else {
            current
                .progress_mm
                .saturating_sub(step)
                .max(target.progress_mm)
        };
        Ok((
            LanePosition {
                lane: self.id,
                progress_mm: next,
            },
            next == target.progress_mm,
        ))
    }
}

fn interpolate(start: Vec2Mm, end: Vec2Mm, offset: u64, length: u64) -> Vec2Mm {
    if offset == 0 || length == 0 {
        return start;
    }
    if offset == length {
        return end;
    }
    Vec2Mm::new(
        interpolate_axis(start.x, end.x, offset, length),
        interpolate_axis(start.y, end.y, offset, length),
    )
}

fn interpolate_axis(start: i32, end: i32, offset: u64, length: u64) -> i32 {
    let delta = i128::from(end) - i128::from(start);
    let numerator = delta * i128::from(offset);
    let half = i128::from(length / 2);
    let rounded = if numerator >= 0 {
        (numerator + half) / i128::from(length)
    } else {
        (numerator - half) / i128::from(length)
    };
    i32::try_from(i128::from(start) + rounded).unwrap_or(if rounded < 0 {
        i32::MIN
    } else {
        i32::MAX
    })
}

fn integer_sqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut low = 1_u128;
    let mut high = value / 2 + 1;
    while low <= high {
        let middle = low + (high - low) / 2;
        if middle <= value / middle {
            low = middle + 1;
        } else {
            high = middle - 1;
        }
    }
    high
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneError {
    TooFewPoints,
    PointBudgetExceeded,
    ZeroWidth,
    PointOutOfBounds,
    DuplicateConsecutivePoint,
    LengthOverflow,
    WrongLane,
    ProgressOutOfBounds,
    OutsideCorridor,
}

impl Display for LaneError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TooFewPoints => "lane requires at least two centerline points",
            Self::PointBudgetExceeded => "lane centerline point budget exceeded",
            Self::ZeroWidth => "lane corridor width must be positive",
            Self::PointOutOfBounds => "lane centerline point is outside map bounds",
            Self::DuplicateConsecutivePoint => "lane has duplicate consecutive points",
            Self::LengthOverflow => "lane length arithmetic overflowed",
            Self::WrongLane => "lane position references a different lane",
            Self::ProgressOutOfBounds => "lane progress exceeds the cooked lane length",
            Self::OutsideCorridor => "destination is outside the authored lane corridor",
        })
    }
}

impl Error for LaneError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn bent_lane() -> CompiledLane {
        CompiledLane::compile(
            &LaneSpec {
                id: LaneId(7),
                centerline: vec![
                    Vec2Mm::new(0, 0),
                    Vec2Mm::new(1_000, 0),
                    Vec2Mm::new(1_000, 1_000),
                ],
                half_width_mm: 200,
            },
            MatchConfig::default(),
        )
        .unwrap()
    }

    #[test]
    fn validates_authored_lane_bounds_and_budgets() {
        let config = MatchConfig {
            max_lane_points_per_lane: 2,
            ..MatchConfig::default()
        };
        let too_many = LaneSpec {
            id: LaneId(0),
            centerline: vec![Vec2Mm::new(0, 0), Vec2Mm::new(1, 0), Vec2Mm::new(2, 0)],
            half_width_mm: 1,
        };
        assert_eq!(
            CompiledLane::compile(&too_many, config),
            Err(LaneError::PointBudgetExceeded)
        );

        let duplicate = LaneSpec {
            id: LaneId(0),
            centerline: vec![Vec2Mm::new(0, 0), Vec2Mm::new(0, 0)],
            half_width_mm: 1,
        };
        assert_eq!(
            CompiledLane::compile(&duplicate, MatchConfig::default()),
            Err(LaneError::DuplicateConsecutivePoint)
        );
    }

    #[test]
    fn projects_bends_and_rejects_outside_corridor() {
        let lane = bent_lane();
        assert_eq!(
            lane.project(Vec2Mm::new(500, 100)).unwrap(),
            LanePosition {
                lane: LaneId(7),
                progress_mm: 500,
            }
        );
        assert_eq!(
            lane.project(Vec2Mm::new(900, 500)).unwrap(),
            LanePosition {
                lane: LaneId(7),
                progress_mm: 1_500,
            }
        );
        assert_eq!(
            lane.project(Vec2Mm::new(500, 500)),
            Err(LaneError::OutsideCorridor)
        );
    }

    #[test]
    fn movement_uses_progress_and_arrives_exactly_in_both_directions() {
        let lane = bent_lane();
        let start = LanePosition {
            lane: LaneId(7),
            progress_mm: 900,
        };
        let end = LanePosition {
            lane: LaneId(7),
            progress_mm: 1_100,
        };
        let (next, arrived) = lane.advance_towards(start, end, 150).unwrap();
        assert_eq!(next.progress_mm, 1_050);
        assert!(!arrived);
        let (next, arrived) = lane.advance_towards(next, end, 150).unwrap();
        assert_eq!(next.progress_mm, 1_100);
        assert!(arrived);
        assert_eq!(lane.point_at(next).unwrap(), Vec2Mm::new(1_000, 100));

        let (back, arrived) = lane.advance_towards(next, start, 500).unwrap();
        assert_eq!(back, start);
        assert!(arrived);
    }
}
