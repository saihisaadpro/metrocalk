//! The desktop **wallet** (M7) — the file-backed token ledger the three paid sinks meter against.
//!
//! Wraps the pure [`metrocalk_economy::Ledger`] (balance = deterministic fold) with persistence beside
//! the scene log, the one-time free-tier seed, and the orphan-hold sweep. All operations are on the
//! single desktop user ([`AccountId::User`]); the platform/creator accounts are bookkept inside the
//! ledger. The wallet is **separate** from the scene replay log — replaying the scene never re-charges
//! tokens, and the balance survives reload (so the free grant can't be farmed by relaunching).

use std::path::PathBuf;

use metrocalk_economy::{
    AccountId, Action, HoldId, Ledger, Mtk, PayError, PaymentProvider, Refusal, FREE_GRANT_TOKENS,
};

/// A file-backed token wallet for the local user.
pub struct Wallet {
    ledger: Ledger,
    /// `None` for an in-memory wallet (tests) — nothing is persisted.
    path: Option<PathBuf>,
    /// Whether the most recent persist succeeded. The paid sinks gate **scene**-persistence on this, so
    /// a failed wallet write can never leave a scene op persisted without its charge (no free paid
    /// tier); a crash then errs toward an over-charge (refundable), the conservative direction.
    last_write_ok: bool,
}

impl Wallet {
    /// Open the file-backed wallet at `path`: load the persisted ledger, **release** any orphan hold
    /// (a generation in-flight when the app was killed → refunded, never silently kept), and seed the
    /// one-time free grant **only for a genuinely fresh wallet**. Idempotent across launches.
    ///
    /// A wallet file that is present but **unreadable** (corrupt) is NOT re-seeded — a corrupted file
    /// can't farm the free grant. (A deliberately *deleted* file is treated as fresh and re-seeded — a
    /// local-only attack, acceptable for a desktop free app; a server-side ledger closes it — ADR-018.)
    #[must_use]
    pub fn open(path: PathBuf) -> Self {
        let contents = std::fs::read_to_string(&path).ok();
        let parsed = contents
            .as_ref()
            .and_then(|s| serde_json::from_str::<Ledger>(s).ok());
        let corrupt = contents.is_some() && parsed.is_none();
        let mut w = Wallet {
            ledger: parsed.unwrap_or_default(),
            path: Some(path),
            last_write_ok: true,
        };
        let swept = w.ledger.sweep_open_holds();
        let seeded = if corrupt {
            eprintln!("[wallet] existing wallet file is unreadable — NOT re-seeding the free tier");
            false
        } else {
            w.seed_free_tier()
        };
        if !swept.is_empty() || seeded {
            w.save();
        }
        w
    }

    /// An in-memory wallet for tests — seeded with the free grant, nothing persisted.
    #[must_use]
    pub fn in_memory() -> Self {
        let mut w = Wallet {
            ledger: Ledger::new(),
            path: None,
            last_write_ok: true,
        };
        w.seed_free_tier();
        w
    }

    /// Seed the one-time free-tier grant (idempotent). Returns whether a grant was appended.
    fn seed_free_tier(&mut self) -> bool {
        self.ledger
            .grant_free_tier(AccountId::User, Mtk::from_tokens(FREE_GRANT_TOKENS))
            .is_some()
    }

    /// Persist the ledger **atomically** — write a temp file, then rename — so the wallet file is never
    /// partially written/corrupt (a half-written wallet would deny service or, worse, reset → re-grant
    /// the free tier). Returns whether it persisted (recorded in `last_write_ok`). An in-memory wallet
    /// (no path) is trivially persisted. Best-effort: a write failure never crashes the editor.
    fn save(&mut self) -> bool {
        let ok = match (&self.path, serde_json::to_string(&self.ledger)) {
            (Some(path), Ok(json)) => {
                let tmp = path.with_extension("json.tmp");
                std::fs::write(&tmp, json.as_bytes()).is_ok() && std::fs::rename(&tmp, path).is_ok()
            }
            (None, _) => true,
            (Some(_), Err(_)) => false,
        };
        self.last_write_ok = ok;
        ok
    }

    /// Whether the most recent wallet write persisted — the sinks gate scene-persistence on this.
    #[must_use]
    pub fn last_write_ok(&self) -> bool {
        self.last_write_ok
    }

    /// The user's settled balance, in whole tokens (for UX).
    #[must_use]
    pub fn balance_tokens(&self) -> u32 {
        self.ledger.balance(&AccountId::User).whole_tokens()
    }

    /// The user's spendable balance (settled minus open holds), in whole tokens.
    #[must_use]
    pub fn available_tokens(&self) -> u32 {
        self.ledger.available(&AccountId::User).whole_tokens()
    }

    /// Whether the user can afford `action` right now.
    #[must_use]
    pub fn can_afford(&self, action: &Action) -> bool {
        self.ledger.available(&AccountId::User) >= action.cost()
    }

    /// Charge a SYNCHRONOUS action (marketplace buy or AI-edit): debit-on-success with the accrual
    /// split, or a graceful [`Refusal`] when broke. Returns the cost in whole tokens.
    ///
    /// # Errors
    /// [`Refusal`] when the user can't afford the action.
    pub fn charge(&mut self, action: &Action, ref_id: &str) -> Result<u32, Refusal> {
        let cost = self.ledger.charge(&AccountId::User, action, ref_id)?;
        self.save();
        Ok(cost.whole_tokens())
    }

    /// Reserve for a generation (the async window) — fences the cost off `available` immediately.
    ///
    /// # Errors
    /// [`Refusal`] when the user can't afford a generation.
    pub fn reserve_generate(&mut self, ref_id: &str) -> Result<HoldId, Refusal> {
        let hold = self
            .ledger
            .reserve(&AccountId::User, &Action::Generate, ref_id)?;
        self.save();
        Ok(hold)
    }

    /// Capture a generation hold as a realized spend (generation succeeded). Returns `true` only if it
    /// settled in-memory AND **persisted** — the caller persists the scene generation only on `true`, so
    /// a wallet-write failure never leaves a free (or charged-but-unrecorded) generation. Idempotent (a
    /// no-op `false` if the hold was already settled/released — never double-charges).
    pub fn settle(&mut self, hold: HoldId, ref_id: &str) -> bool {
        self.ledger.settle(hold, ref_id).is_some() && self.save()
    }

    /// Release a generation hold (generation failed/rejected) — the reserved tokens return to
    /// `available`, never charged. Returns `true` only if released AND persisted. Idempotent.
    pub fn release(&mut self, hold: HoldId, ref_id: &str) -> bool {
        self.ledger.release(hold, ref_id).is_some() && self.save()
    }

    /// Top up via a payment provider (sandbox by default — no real money). Charges `cents`, then grants
    /// the returned tokens via a ledger entry. Returns the granted token count.
    ///
    /// # Errors
    /// [`PayError`] when the provider declines or is an unconfigured go-live seam.
    pub fn top_up(
        &mut self,
        provider: &dyn PaymentProvider,
        cents: u32,
        ref_id: &str,
    ) -> Result<u32, PayError> {
        let receipt = provider.charge(cents, ref_id)?;
        self.ledger
            .top_up(AccountId::User, receipt.tokens, &receipt.provider_ref);
        self.save();
        Ok(receipt.tokens)
    }

    /// The underlying ledger (for audit / tests).
    #[must_use]
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_wallet_is_seeded_with_the_free_grant_once() {
        let w = Wallet::in_memory();
        assert_eq!(w.balance_tokens(), FREE_GRANT_TOKENS);
        assert_eq!(w.available_tokens(), FREE_GRANT_TOKENS);
    }

    #[test]
    fn an_edit_debits_two_tokens_on_success() {
        let mut w = Wallet::in_memory();
        let cost = w.charge(&Action::Edit, "edit-1").unwrap();
        assert_eq!(cost, 2);
        assert_eq!(w.balance_tokens(), FREE_GRANT_TOKENS - 2);
    }

    #[test]
    fn a_generation_reserve_then_settle_charges_ten_once() {
        let mut w = Wallet::in_memory();
        let hold = w.reserve_generate("gen-1").unwrap();
        assert_eq!(
            w.available_tokens(),
            FREE_GRANT_TOKENS - 10,
            "reserve fences 10"
        );
        assert_eq!(
            w.balance_tokens(),
            FREE_GRANT_TOKENS,
            "but balance not yet spent"
        );
        assert!(w.settle(hold, "gen-1"));
        assert_eq!(w.balance_tokens(), FREE_GRANT_TOKENS - 10);
        assert!(!w.settle(hold, "gen-1"), "double settle is a no-op");
        assert_eq!(w.balance_tokens(), FREE_GRANT_TOKENS - 10);
    }

    #[test]
    fn a_failed_generation_releases_the_hold_and_never_charges() {
        let mut w = Wallet::in_memory();
        let hold = w.reserve_generate("gen-1").unwrap();
        assert!(w.release(hold, "gen-1"));
        assert_eq!(
            w.balance_tokens(),
            FREE_GRANT_TOKENS,
            "never charged on failure"
        );
        assert_eq!(w.available_tokens(), FREE_GRANT_TOKENS);
    }

    #[test]
    fn charges_are_refused_gracefully_when_broke() {
        let mut w = Wallet::in_memory();
        // Drain the free grant (30 tokens) with edits (2 each) → 15 edits.
        for i in 0..15 {
            w.charge(&Action::Edit, &format!("e{i}")).unwrap();
        }
        assert_eq!(w.balance_tokens(), 0);
        // A generation now refuses gracefully.
        let refusal = w.reserve_generate("gen-broke").unwrap_err();
        assert_eq!(refusal.needed, Mtk::from_tokens(10));
        assert_eq!(refusal.available, Mtk::ZERO);
    }

    #[test]
    fn a_buy_pays_the_creator_seventy_percent() {
        let mut w = Wallet::in_memory();
        let buy = Action::Buy {
            price_tokens: 4,
            creator: Some("forge".to_string()),
        };
        assert_eq!(w.charge(&buy, "buy-1").unwrap(), 4);
        assert_eq!(w.balance_tokens(), FREE_GRANT_TOKENS - 4);
        assert_eq!(
            w.ledger().balance(&AccountId::Creator("forge".to_string())),
            Mtk(2800),
            "creator accrues 70%"
        );
    }

    #[test]
    fn a_fresh_file_wallet_seeds_once_and_persists_atomically_across_reopen() {
        let path = std::env::temp_dir().join("metrocalk-wallet-fresh-test.json");
        let _ = std::fs::remove_file(&path);
        {
            let mut w = Wallet::open(path.clone());
            assert_eq!(w.balance_tokens(), FREE_GRANT_TOKENS, "fresh → free grant");
            w.charge(&Action::Edit, "e1").unwrap(); // spend 2, persisted
            assert_eq!(w.balance_tokens(), FREE_GRANT_TOKENS - 2);
        }
        // Reopen: the spend survived AND the free tier is NOT re-granted (un-farmable by relaunch).
        let w2 = Wallet::open(path.clone());
        assert_eq!(
            w2.balance_tokens(),
            FREE_GRANT_TOKENS - 2,
            "balance persisted; no re-grant on reopen"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_corrupt_wallet_file_is_not_re_seeded_with_the_free_tier() {
        let path = std::env::temp_dir().join("metrocalk-wallet-corrupt-test.json");
        std::fs::write(&path, b"{ this is not valid ledger json").unwrap();
        let w = Wallet::open(path.clone());
        assert_eq!(
            w.balance_tokens(),
            0,
            "a corrupt wallet must not mint a fresh free grant (no corruption-farming)"
        );
        let _ = std::fs::remove_file(&path);
    }
}
