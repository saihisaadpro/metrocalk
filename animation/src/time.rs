// Conversion is an explicit authoring-boundary operation; the range is checked before f64 -> i64 and
// animation evaluation stays in integer ticks.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use serde::{Deserialize, Serialize};

/// An exact integer position in an animation time base.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Tick(pub i64);

impl Tick {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    #[must_use]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

/// Exact clock declaration shared by every track in a sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimeBase {
    /// Integer ticks in one second. Must be non-zero.
    pub ticks_per_second: u32,
}

impl TimeBase {
    pub const FILM_24: Self = Self::new(24_000);
    pub const GAME_60: Self = Self::new(60_000);

    #[must_use]
    pub const fn new(ticks_per_second: u32) -> Self {
        Self { ticks_per_second }
    }

    /// Converts seconds to the nearest tick. Authoring boundaries perform this once; evaluation remains
    /// integer-only until a segment interpolation factor is needed.
    pub fn from_seconds(self, seconds: f64) -> Result<Tick, TimeError> {
        if self.ticks_per_second == 0 {
            return Err(TimeError::ZeroRate);
        }
        if !seconds.is_finite() {
            return Err(TimeError::NonFiniteSeconds);
        }
        let ticks = seconds * f64::from(self.ticks_per_second);
        if ticks < i64::MIN as f64 || ticks > i64::MAX as f64 {
            return Err(TimeError::OutOfRange);
        }
        Ok(Tick(ticks.round() as i64))
    }

    pub fn to_seconds(self, tick: Tick) -> Result<f64, TimeError> {
        if self.ticks_per_second == 0 {
            return Err(TimeError::ZeroRate);
        }
        Ok(tick.0 as f64 / f64::from(self.ticks_per_second))
    }

    /// Convert an integer tick to another clock using checked integer rational arithmetic and
    /// nearest-tick rounding. Runtime graph sources use this at admission so imported high-resolution
    /// clocks preserve wall time without leaking mixed tick domains into one deterministic playhead.
    pub fn convert_tick(self, tick: Tick, target: Self) -> Result<Tick, TimeError> {
        if self.ticks_per_second == 0 || target.ticks_per_second == 0 {
            return Err(TimeError::ZeroRate);
        }
        if self == target {
            return Ok(tick);
        }
        let numerator = i128::from(tick.0) * i128::from(target.ticks_per_second);
        let denominator = i128::from(self.ticks_per_second);
        let quotient = numerator / denominator;
        let remainder = numerator % denominator;
        let rounded = if remainder.abs() * 2 >= denominator {
            quotient + numerator.signum()
        } else {
            quotient
        };
        i64::try_from(rounded)
            .map(Tick)
            .map_err(|_| TimeError::OutOfRange)
    }
}

impl Default for TimeBase {
    fn default() -> Self {
        Self::GAME_60
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeError {
    ZeroRate,
    NonFiniteSeconds,
    OutOfRange,
    PrecisionLoss,
}

impl std::fmt::Display for TimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ZeroRate => "time base must have at least one tick per second",
            Self::NonFiniteSeconds => "seconds must be finite",
            Self::OutOfRange => "seconds cannot be represented by an i64 tick",
            Self::PrecisionLoss => {
                "the target time base cannot represent distinct source timing exactly enough"
            }
        })
    }
}

impl std::error::Error for TimeError {}
