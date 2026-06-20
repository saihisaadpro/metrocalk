//! The token ledger — an **append-only, auditable** journal whose balance is a **deterministic fold**.
//!
//! The economic analog of the scene commit pipeline (invariant 3): the ONLY way to change a balance is
//! to append an [`Event`]; [`Ledger::balance`]/[`Ledger::held`]/[`Ledger::available`] are pure folds
//! over the log, so the ledger replays byte-identically (no clock, no RNG). Every spend is a TRANSFER
//! between accounts (never a burn), so total tokens are conserved: Σ balances == Σ grants.

use serde::{Deserialize, Serialize};

use crate::model::{self, Action};
use crate::units::Mtk;

/// An account in the economy. The single desktop user is [`AccountId::User`]; marketplace revenue
/// accrues to [`AccountId::Platform`] and to a per-creator [`AccountId::Creator`] (the namespace that
/// authored a marketplace entry).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccountId {
    /// The local desktop user (the buyer/spender).
    User,
    /// Platform revenue (generation, the platform cut of a buy, edits).
    Platform,
    /// A creator's payout-accrual account, keyed by their namespace.
    Creator(String),
}

/// Why an entry exists — the audit label (the specific entity/entry is in `ref_id`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reason {
    /// The one-time free-tier seed.
    FreeTier,
    /// A top-up via a payment provider.
    TopUp {
        /// The provider's reference (audit).
        provider_ref: String,
    },
    /// A generation spend.
    Generate,
    /// A marketplace buy spend.
    Buy,
    /// An AI-edit spend.
    Edit,
}

/// A reservation handle — the [`Seq`] of its [`Event::Hold`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HoldId(pub u64);

/// A monotonic log index, assigned by [`Ledger::append`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Seq(pub u64);

/// One immutable economic event. Self-describing so the `balance` fold stays local (no cross-entry
/// lookups). `ref_id` correlates the entries of one transaction (a buy's debit + its two accruals; a
/// generation's hold + its settle/release).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    /// Tokens enter the system (free-tier seed or a top-up) — the only mint.
    Grant {
        /// Credited account.
        to: AccountId,
        /// Amount granted.
        amount: Mtk,
        /// Audit label.
        reason: Reason,
    },
    /// An immediate spend (synchronous sinks: buy, edit).
    Debit {
        /// Debited account.
        from: AccountId,
        /// Amount.
        amount: Mtk,
        /// Audit label.
        reason: Reason,
        /// Transaction correlation id.
        ref_id: String,
    },
    /// A payout/revenue credit (creator or platform share of a spend).
    Accrue {
        /// Credited account.
        to: AccountId,
        /// Amount.
        amount: Mtk,
        /// Transaction correlation id.
        ref_id: String,
    },
    /// A reservation against future spend (the async generation in-flight window).
    Hold {
        /// Reserving account.
        account: AccountId,
        /// Amount held.
        amount: Mtk,
        /// Audit label.
        reason: Reason,
        /// Transaction correlation id.
        ref_id: String,
    },
    /// Capture a hold as a realized spend (records the debited account + amount, self-contained).
    Settle {
        /// The captured hold.
        hold: HoldId,
        /// Debited account (the hold's owner).
        from: AccountId,
        /// Amount captured.
        amount: Mtk,
        /// Transaction correlation id.
        ref_id: String,
    },
    /// Cancel a hold — the reserved funds return to `available`, never charged (a refund-by-non-charge).
    Release {
        /// The cancelled hold.
        hold: HoldId,
        /// Transaction correlation id.
        ref_id: String,
    },
}

/// One persisted ledger entry — a sequence number + its event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Monotonic log index.
    pub seq: Seq,
    /// The event.
    pub event: Event,
}

/// A gracefully-refused charge — the honest "top up?" UX (never a silent free pass or unexplained block).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// What the action costs.
    pub needed: Mtk,
    /// What the account could spend.
    pub available: Mtk,
}

/// The append-only token ledger.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    entries: Vec<Entry>,
}

/// The audit [`Reason`] for a spend action.
fn reason_for(action: &Action) -> Reason {
    match action {
        Action::Generate => Reason::Generate,
        Action::Edit => Reason::Edit,
        Action::Buy { .. } => Reason::Buy,
    }
}

impl Ledger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A ledger restored from a replayed log (the persisted entries).
    #[must_use]
    pub fn from_entries(entries: Vec<Entry>) -> Self {
        Self { entries }
    }

    /// The append-only log (for persistence / audit).
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the log is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The SOLE mutator — append one event, assigning the next [`Seq`]. Returns its seq.
    pub fn append(&mut self, event: Event) -> Seq {
        let seq = Seq(u64::try_from(self.entries.len()).unwrap_or(u64::MAX));
        self.entries.push(Entry { seq, event });
        seq
    }

    /// The settled balance of `acct` — a deterministic fold: `+ Grant + Accrue − Debit − Settle`. Holds
    /// do NOT reduce the balance (the funds are still owned), only `available`.
    #[must_use]
    pub fn balance(&self, acct: &AccountId) -> Mtk {
        let mut bal: i128 = 0;
        for e in &self.entries {
            match &e.event {
                Event::Grant { to, amount, .. } | Event::Accrue { to, amount, .. }
                    if to == acct =>
                {
                    bal += i128::from(amount.get());
                }
                Event::Debit { from, amount, .. } | Event::Settle { from, amount, .. }
                    if from == acct =>
                {
                    bal -= i128::from(amount.get());
                }
                _ => {}
            }
        }
        debug_assert!(
            bal >= 0,
            "ledger balance went negative — a charge bypassed the affordability check"
        );
        Mtk(u64::try_from(bal.max(0)).unwrap_or(0))
    }

    /// The total still OPEN holds for `acct` (a hold with no matching Settle/Release).
    #[must_use]
    pub fn held(&self, acct: &AccountId) -> Mtk {
        // One pass to collect closed hold ids, then sum the still-open holds for `acct` — O(n), not the
        // O(holds·n) of a per-hold `is_hold_open` scan (which every interactive reserve would pay as the
        // ledger grows).
        let closed = self.closed_holds();
        let mut total: u64 = 0;
        for e in &self.entries {
            if let Event::Hold {
                account, amount, ..
            } = &e.event
            {
                if account == acct && !closed.contains(&e.seq.0) {
                    total += amount.get();
                }
            }
        }
        Mtk(total)
    }

    /// The ids of holds that have been settled or released (closed) — one pass.
    fn closed_holds(&self) -> std::collections::HashSet<u64> {
        self.entries
            .iter()
            .filter_map(|e| match &e.event {
                Event::Settle { hold, .. } | Event::Release { hold, .. } => Some(hold.0),
                _ => None,
            })
            .collect()
    }

    /// What `acct` can actually spend right now: `balance − held` (open reservations are fenced off —
    /// this is what every charge/reserve checks, and what defeats free-tier-via-race).
    #[must_use]
    pub fn available(&self, acct: &AccountId) -> Mtk {
        self.balance(acct).saturating_sub(self.held(acct))
    }

    /// Whether `hold` is open (the Hold exists and no later Settle/Release names it).
    #[must_use]
    pub fn is_hold_open(&self, hold: HoldId) -> bool {
        let exists = self
            .entries
            .iter()
            .any(|e| e.seq.0 == hold.0 && matches!(e.event, Event::Hold { .. }));
        if !exists {
            return false;
        }
        let closed = self.entries.iter().any(|e| {
            matches!(&e.event,
                Event::Settle { hold: h, .. } | Event::Release { hold: h, .. } if *h == hold)
        });
        !closed
    }

    /// The account + amount of a hold (whether or not it is still open).
    #[must_use]
    pub fn hold_info(&self, hold: HoldId) -> Option<(AccountId, Mtk)> {
        self.entries.iter().find_map(|e| match &e.event {
            Event::Hold {
                account, amount, ..
            } if e.seq.0 == hold.0 => Some((account.clone(), *amount)),
            _ => None,
        })
    }

    /// Grant tokens (the only mint). Returns the entry seq.
    pub fn grant(&mut self, to: AccountId, amount: Mtk, reason: Reason) -> Seq {
        self.append(Event::Grant { to, amount, reason })
    }

    /// Seed the one-time free-tier grant — IDEMPOTENT: only if no prior free-tier grant exists for the
    /// account, so relaunch/replay never re-grants (the free-tier-farming guard). Returns the new seq,
    /// or `None` if already granted.
    pub fn grant_free_tier(&mut self, to: AccountId, amount: Mtk) -> Option<Seq> {
        let already = self.entries.iter().any(|e| {
            matches!(&e.event,
                Event::Grant { to: t, reason: Reason::FreeTier, .. } if *t == to)
        });
        if already {
            return None;
        }
        Some(self.grant(to, amount, Reason::FreeTier))
    }

    /// Grant tokens from a payment-provider top-up.
    pub fn top_up(&mut self, to: AccountId, tokens: u32, provider_ref: &str) -> Seq {
        self.grant(
            to,
            Mtk::from_tokens(tokens),
            Reason::TopUp {
                provider_ref: provider_ref.to_string(),
            },
        )
    }

    /// Charge a SYNCHRONOUS action (buy or edit). Checks `available` FIRST; on success appends a Debit
    /// and the accrual split as one `ref_id`-correlated transaction; on insufficient balance refuses
    /// gracefully (nothing appended). A zero-cost action (a free/price-less marketplace entry) is a
    /// no-op `Ok(ZERO)` that never touches the ledger.
    ///
    /// Generation is async — use [`Ledger::reserve`] + [`Ledger::settle`]/[`Ledger::release`] instead.
    ///
    /// # Errors
    /// [`Refusal`] when `available < cost`.
    pub fn charge(
        &mut self,
        account: &AccountId,
        action: &Action,
        ref_id: &str,
    ) -> Result<Mtk, Refusal> {
        let cost = action.cost();
        if cost == Mtk::ZERO {
            return Ok(Mtk::ZERO); // a free entry never touches the ledger
        }
        let available = self.available(account);
        if available < cost {
            return Err(Refusal {
                needed: cost,
                available,
            });
        }
        self.append(Event::Debit {
            from: account.clone(),
            amount: cost,
            reason: reason_for(action),
            ref_id: ref_id.to_string(),
        });
        self.accrue_spend(action, cost, ref_id);
        Ok(cost)
    }

    /// Reserve for the async generation window. Checks `available` FIRST (the fence that defeats
    /// free-tier-via-race: the hold drops `available` the instant the request is accepted). Returns a
    /// [`HoldId`], or refuses gracefully.
    ///
    /// # Errors
    /// [`Refusal`] when `available < cost`.
    pub fn reserve(
        &mut self,
        account: &AccountId,
        action: &Action,
        ref_id: &str,
    ) -> Result<HoldId, Refusal> {
        let cost = action.cost();
        let available = self.available(account);
        if available < cost {
            return Err(Refusal {
                needed: cost,
                available,
            });
        }
        let seq = self.append(Event::Hold {
            account: account.clone(),
            amount: cost,
            reason: reason_for(action),
            ref_id: ref_id.to_string(),
        });
        Ok(HoldId(seq.0))
    }

    /// Capture a hold as a realized spend (generation succeeded) — appends Settle + the platform
    /// accrual. IDEMPOTENT: a no-op (returns `None`) if the hold is already settled/released, so a
    /// double completion cannot double-charge.
    pub fn settle(&mut self, hold: HoldId, ref_id: &str) -> Option<Seq> {
        if !self.is_hold_open(hold) {
            return None;
        }
        let (from, amount) = self.hold_info(hold)?;
        let seq = self.append(Event::Settle {
            hold,
            from,
            amount,
            ref_id: ref_id.to_string(),
        });
        // A fresh generation is 100% platform revenue (no creator).
        self.append(Event::Accrue {
            to: AccountId::Platform,
            amount,
            ref_id: ref_id.to_string(),
        });
        Some(seq)
    }

    /// Cancel a hold (generation failed/rejected) — the reserved funds return to `available`, never
    /// charged. IDEMPOTENT (a no-op if already settled/released).
    pub fn release(&mut self, hold: HoldId, ref_id: &str) -> Option<Seq> {
        if !self.is_hold_open(hold) {
            return None;
        }
        Some(self.append(Event::Release {
            hold,
            ref_id: ref_id.to_string(),
        }))
    }

    /// Release every still-open hold — the startup/replay sweep, so a generation in-flight when the app
    /// was killed is refunded (never silently kept). Returns the swept hold ids.
    pub fn sweep_open_holds(&mut self) -> Vec<HoldId> {
        let closed = self.closed_holds();
        let open: Vec<HoldId> = self
            .entries
            .iter()
            .filter_map(|e| match &e.event {
                Event::Hold { .. } if !closed.contains(&e.seq.0) => Some(HoldId(e.seq.0)),
                _ => None,
            })
            .collect();
        for h in &open {
            self.append(Event::Release {
                hold: *h,
                ref_id: "sweep".to_string(),
            });
        }
        open
    }

    /// Append the revenue accrual(s) for a spend: a buy splits ~70/30 creator/platform; an edit (or a
    /// buy with no creator namespace) is 100% platform.
    fn accrue_spend(&mut self, action: &Action, cost: Mtk, ref_id: &str) {
        match action {
            Action::Buy {
                creator: Some(ns), ..
            } => {
                let (creator_mt, platform_mt) = model::split(cost);
                self.append(Event::Accrue {
                    to: AccountId::Creator(ns.clone()),
                    amount: creator_mt,
                    ref_id: ref_id.to_string(),
                });
                self.append(Event::Accrue {
                    to: AccountId::Platform,
                    amount: platform_mt,
                    ref_id: ref_id.to_string(),
                });
            }
            // Edit, or a buy with no creator namespace → 100% platform revenue.
            _ => {
                self.append(Event::Accrue {
                    to: AccountId::Platform,
                    amount: cost,
                    ref_id: ref_id.to_string(),
                });
            }
        }
    }
}
