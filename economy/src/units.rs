//! The unit of account: **milli-tokens**.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Milli-tokens per displayed token. Internal amounts are in milli-tokens so the marketplace 70/30
/// payout split is EXACT (70% of a 4-token buy = 2.8 tokens = 2800 mt) and token-conserving — no
/// fraction is ever minted or burned by rounding.
pub const MT_PER_TOKEN: u64 = 1000;

/// US cents per token (ADR-004: $10 ≈ 100 tokens ⇒ 1000¢ / 100 = 10¢/token).
pub const CENTS_PER_TOKEN: u32 = 10;

/// An amount of credit in **milli-tokens** (1/1000 of a displayed token). Unsigned: a balance never
/// goes negative by construction (every spend is affordability-checked first).
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug, Serialize, Deserialize,
)]
pub struct Mtk(pub u64);

impl Mtk {
    /// Zero credit.
    pub const ZERO: Mtk = Mtk(0);

    /// `tokens` whole displayed tokens, in milli-tokens.
    #[must_use]
    pub fn from_tokens(tokens: u32) -> Self {
        Mtk(u64::from(tokens) * MT_PER_TOKEN)
    }

    /// The raw milli-token count.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }

    /// Whole displayed tokens (floor) — for UX ("≈ N tokens").
    #[must_use]
    pub fn whole_tokens(self) -> u32 {
        u32::try_from(self.0 / MT_PER_TOKEN).unwrap_or(u32::MAX)
    }

    /// Saturating subtraction (a balance floors at zero).
    #[must_use]
    pub fn saturating_sub(self, rhs: Mtk) -> Mtk {
        Mtk(self.0.saturating_sub(rhs.0))
    }
}

impl std::ops::Add for Mtk {
    type Output = Mtk;
    fn add(self, rhs: Mtk) -> Mtk {
        Mtk(self.0 + rhs.0)
    }
}

impl std::ops::AddAssign for Mtk {
    fn add_assign(&mut self, rhs: Mtk) {
        self.0 += rhs.0;
    }
}

impl fmt::Display for Mtk {
    /// e.g. `2800` → `2.8 tokens`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{} tokens",
            self.0 / MT_PER_TOKEN,
            (self.0 % MT_PER_TOKEN) / 100
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_round_trip_through_milli_tokens() {
        for t in [0u32, 1, 2, 3, 4, 10, 30, 100, 999] {
            assert_eq!(Mtk::from_tokens(t).whole_tokens(), t, "round-trip token↔mt");
            assert_eq!(Mtk::from_tokens(t).get(), u64::from(t) * MT_PER_TOKEN);
        }
    }

    #[test]
    fn display_shows_sub_token_precision() {
        assert_eq!(Mtk(2800).to_string(), "2.8 tokens");
        assert_eq!(Mtk(10_000).to_string(), "10.0 tokens");
        assert_eq!(Mtk(1200).to_string(), "1.2 tokens");
    }
}
