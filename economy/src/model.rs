//! The cost model + the pricing invariant + the creator payout split (ADR-004 — the spec).
//!
//! All numbers trace to ADR-004 and are kept in ONE place. Costs are in milli-tokens so the 70/30
//! payout split is exact and token-conserving.

use crate::units::{Mtk, CENTS_PER_TOKEN};

/// Fresh text-to-3D generation ≈ 10 tokens (covers provider GPU cost with margin).
pub const GENERATE_TOKENS: u32 = 10;
/// LLM edit of an existing asset ("make it rustier") — ADR-004: 1–2 tokens. We charge the top of the
/// band (2) as the canonical edit cost; a cheaper trivial edit is a future refinement.
pub const EDIT_TOKENS: u32 = 2;
/// A comparable marketplace asset is priced 2–4 tokens (ADR-004); the catalog stays within this band.
pub const MIN_MARKETPLACE_PRICE: u32 = 2;
/// Upper bound of a comparable marketplace price (ADR-004).
pub const MAX_MARKETPLACE_PRICE: u32 = 4;
/// Starter pack: $10 ≈ 100 tokens (ADR-004).
pub const STARTER_PACK_TOKENS: u32 = 100;
/// New accounts get "a few free generations" (ADR-004) — three, granted once.
pub const FREE_GRANT_TOKENS: u32 = GENERATE_TOKENS * 3;

/// Creator share of a marketplace buy, in basis points (ADR-004: creator keeps ~70%).
const CREATOR_BPS: u64 = 7000;
/// Basis-point denominator.
const TOTAL_BPS: u64 = 10_000;

/// A metered action and its token cost class (ADR-004). Generation is the ASYNC sink (reserve→settle);
/// buy and edit are SYNCHRONOUS (atomic debit). A buy carries its price + the credited creator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Fresh text-to-3D generation (≈10 tokens, platform revenue, async).
    Generate,
    /// A marketplace buy: `price_tokens` (2–4), split ~70/30 to `creator`/platform. `price_tokens == 0`
    /// (a price-less entry) is a FREE buy — it never touches the ledger.
    Buy {
        /// The entry's token price (0 ⇒ free).
        price_tokens: u32,
        /// The credited creator (an entry id's namespace), or `None` ⇒ all to the platform.
        creator: Option<String>,
    },
    /// An LLM edit of an existing asset (1–2 tokens, platform revenue, sync).
    Edit,
}

impl Action {
    /// The cost of this action, in milli-tokens.
    #[must_use]
    pub fn cost(&self) -> Mtk {
        cost(self)
    }
}

/// The cost of an action, in milli-tokens (the one cost surface).
#[must_use]
pub fn cost(action: &Action) -> Mtk {
    match action {
        Action::Generate => Mtk::from_tokens(GENERATE_TOKENS),
        Action::Edit => Mtk::from_tokens(EDIT_TOKENS),
        Action::Buy { price_tokens, .. } => Mtk::from_tokens(*price_tokens),
    }
}

/// The ADR-004 pricing invariant: **buy + edit < regenerate**, so the resolution order
/// local→marketplace→generate is enforced by economics, not policy. Holds for every comparable price.
#[must_use]
pub fn buy_plus_edit_beats_regenerate(price_tokens: u32) -> bool {
    price_tokens + EDIT_TOKENS < GENERATE_TOKENS
}

/// The creator/platform split of a buy, in milli-tokens — EXACT and token-conserving: `creator +
/// platform == price`; the rounding remainder (if any) goes to the platform, so nothing is minted or
/// burned.
#[must_use]
pub fn split(price: Mtk) -> (Mtk, Mtk) {
    let creator = Mtk(price.get() * CREATOR_BPS / TOTAL_BPS);
    let platform = Mtk(price.get() - creator.get());
    (creator, platform)
}

/// The creator credited for a marketplace entry = its id's **namespace prefix** (`forge:rusty-sword` →
/// `forge`). An entry id with no `:`-namespace (or an empty one) has no creator — its whole price
/// accrues to the platform, never to an empty-string creator.
#[must_use]
pub fn creator_of(entry_id: &str) -> Option<String> {
    entry_id
        .split_once(':')
        .map(|(ns, _)| ns.to_string())
        .filter(|ns| !ns.is_empty())
}

/// Tokens granted by a top-up of `cents` ($10 ⇒ 100 tokens).
#[must_use]
pub fn tokens_for_cents(cents: u32) -> u32 {
    cents / CENTS_PER_TOKEN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buy_plus_edit_is_always_cheaper_than_regenerate() {
        // The ADR-004 core economic guarantee, across the whole comparable-price band (a property test).
        for price in MIN_MARKETPLACE_PRICE..=MAX_MARKETPLACE_PRICE {
            assert!(
                buy_plus_edit_beats_regenerate(price),
                "buy({price}) + edit({EDIT_TOKENS}) must be < regenerate({GENERATE_TOKENS})"
            );
        }
        // And it must hold at the very top of the band by construction (a compile-time guarantee).
        const { assert!(MAX_MARKETPLACE_PRICE + EDIT_TOKENS < GENERATE_TOKENS) };
    }

    #[test]
    fn payout_split_is_exact_and_conserving() {
        for price in MIN_MARKETPLACE_PRICE..=MAX_MARKETPLACE_PRICE {
            let p = Mtk::from_tokens(price);
            let (creator, platform) = split(p);
            assert_eq!(
                creator.get() + platform.get(),
                p.get(),
                "no token minted or burned by the split"
            );
            assert_eq!(
                creator.get(),
                u64::from(price) * 700,
                "creator is exactly 70%"
            );
            assert_eq!(
                platform.get(),
                u64::from(price) * 300,
                "platform is exactly 30%"
            );
            assert!(creator >= platform, "creator keeps the majority");
        }
    }

    #[test]
    fn creator_attribution_is_the_namespace_prefix() {
        assert_eq!(creator_of("forge:rusty-sword").as_deref(), Some("forge"));
        assert_eq!(creator_of("acme:companion-drone").as_deref(), Some("acme"));
        assert_eq!(creator_of("no-namespace").as_deref(), None);
        assert_eq!(creator_of(":leading-colon").as_deref(), None);
    }

    #[test]
    fn ten_dollars_is_one_hundred_tokens() {
        assert_eq!(tokens_for_cents(1000), STARTER_PACK_TOKENS);
    }
}
