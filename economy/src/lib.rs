//! `metrocalk-economy` — the M7 token economy (ADR-004/005): an append-only token **ledger** (balance =
//! deterministic fold), the **cost model** + the `buy + edit < regenerate` pricing invariant, **creator
//! payout** accounting (~70%, token-conserving), and the **payment-provider seam** (sandbox + go-live).
//!
//! Pure + wasm-portable; the FREE ENGINE (`core`/`ecs`) never depends on this crate — the crate-graph
//! edge IS the free/offline-path proof (a local build/bind/describe-local has no symbol for a debit).
//! **No real money moves here** — the real payment rail is a documented go-live seam, sandbox only.
//!
//! The economic analog of the scene commit pipeline (invariant 3): the ONLY way to change a balance is
//! to append an [`Event`]; `balance`/`held`/`available` are pure folds, so the ledger replays
//! byte-identically. Every spend is a TRANSFER between accounts (never a burn) ⇒ tokens are conserved.

pub mod ledger;
pub mod model;
pub mod payment;
pub mod units;

pub use ledger::{AccountId, Entry, Event, HoldId, Ledger, Reason, Refusal, Seq};
pub use model::{
    buy_plus_edit_beats_regenerate, cost, creator_of, split, tokens_for_cents, Action, EDIT_TOKENS,
    FREE_GRANT_TOKENS, GENERATE_TOKENS, MAX_MARKETPLACE_PRICE, MIN_MARKETPLACE_PRICE,
    STARTER_PACK_TOKENS,
};
pub use payment::{
    select_provider, PayError, PaymentProvider, SandboxProvider, StripeProvider, TopUpReceipt,
};
pub use units::{Mtk, CENTS_PER_TOKEN, MT_PER_TOKEN};
