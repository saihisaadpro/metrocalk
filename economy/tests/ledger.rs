//! The token-economy end-to-end through the public ledger API (ADR-004 M7): grants, the deterministic
//! fold, reserve→settle/release for the async generation window, the buy 70/30 split + conservation,
//! the buy+edit<regenerate pricing invariant, the idempotent free-tier seed, the sandbox top-up→grant,
//! and graceful refusal when broke. No real money moves (sandbox only).

use metrocalk_economy::{
    cost, AccountId, Action, Ledger, Mtk, PaymentProvider, SandboxProvider, FREE_GRANT_TOKENS,
    GENERATE_TOKENS,
};

/// Sum of every account's balance — used to assert global token conservation (Σ balances == Σ grants).
fn total_across_accounts(l: &Ledger, creators: &[&str]) -> u64 {
    let mut total = l.balance(&AccountId::User).get() + l.balance(&AccountId::Platform).get();
    for c in creators {
        total += l.balance(&AccountId::Creator((*c).to_string())).get();
    }
    total
}

#[test]
fn grant_debit_topup_balance_is_a_deterministic_fold_that_replays() {
    let mut l = Ledger::new();
    l.grant_free_tier(AccountId::User, Mtk::from_tokens(FREE_GRANT_TOKENS))
        .expect("seed");
    assert_eq!(l.balance(&AccountId::User), Mtk::from_tokens(30));

    // An edit debit (2 tokens) → platform accrues 2.
    l.charge(&AccountId::User, &Action::Edit, "edit-1").unwrap();
    assert_eq!(l.balance(&AccountId::User), Mtk::from_tokens(28));
    assert_eq!(l.balance(&AccountId::Platform), Mtk::from_tokens(2));

    // A top-up (100 tokens).
    l.top_up(AccountId::User, 100, "sandbox-1");
    assert_eq!(l.balance(&AccountId::User), Mtk::from_tokens(128));

    // The balance is a pure fold over the append-only log: serialize → deserialize → fold → identical.
    let json = serde_json::to_string(l.entries()).unwrap();
    let entries: Vec<metrocalk_economy::Entry> = serde_json::from_str(&json).unwrap();
    let replayed = Ledger::from_entries(entries);
    assert_eq!(
        replayed.balance(&AccountId::User),
        l.balance(&AccountId::User),
        "an append-only log replays to the same balance"
    );
    assert_eq!(
        replayed.balance(&AccountId::Platform),
        l.balance(&AccountId::Platform)
    );
}

#[test]
fn a_negative_debit_is_refused_gracefully_never_overdraws() {
    let mut l = Ledger::new();
    l.grant(
        AccountId::User,
        Mtk::from_tokens(1),
        metrocalk_economy::Reason::FreeTier,
    );
    // A generation (10) the user can't afford → graceful refusal, nothing appended.
    let before = l.len();
    let refusal = l
        .reserve(&AccountId::User, &Action::Generate, "gen-1")
        .unwrap_err();
    assert_eq!(refusal.needed, Mtk::from_tokens(GENERATE_TOKENS));
    assert_eq!(refusal.available, Mtk::from_tokens(1));
    assert_eq!(l.len(), before, "a refused charge appends nothing");
    assert_eq!(
        l.balance(&AccountId::User),
        Mtk::from_tokens(1),
        "balance untouched"
    );
}

#[test]
fn generation_reserve_then_settle_charges_exactly_once_not_twice() {
    let mut l = Ledger::new();
    l.top_up(AccountId::User, 100, "sb");

    // Reserve fences the funds the instant the request is accepted (defeats free-tier-via-race).
    let hold = l
        .reserve(&AccountId::User, &Action::Generate, "gen-1")
        .unwrap();
    assert_eq!(
        l.balance(&AccountId::User),
        Mtk::from_tokens(100),
        "hold does not reduce balance"
    );
    assert_eq!(
        l.available(&AccountId::User),
        Mtk::from_tokens(90),
        "but it DOES reduce available by exactly 10"
    );

    // Settle realizes the spend ONCE — available stays down 10, never 20 (the Settle-double-count guard).
    l.settle(hold, "gen-1").expect("settle an open hold");
    assert_eq!(l.balance(&AccountId::User), Mtk::from_tokens(90));
    assert_eq!(l.available(&AccountId::User), Mtk::from_tokens(90));
    assert_eq!(
        l.balance(&AccountId::Platform),
        Mtk::from_tokens(10),
        "generation → platform revenue"
    );

    // Double settle is a no-op (idempotent) — a duplicated completion cannot double-charge.
    assert!(l.settle(hold, "gen-1").is_none());
    assert_eq!(
        l.balance(&AccountId::User),
        Mtk::from_tokens(90),
        "still down only 10"
    );
}

#[test]
fn a_failed_generation_releases_the_hold_and_never_charges() {
    let mut l = Ledger::new();
    l.top_up(AccountId::User, 100, "sb");
    let hold = l
        .reserve(&AccountId::User, &Action::Generate, "gen-1")
        .unwrap();
    assert_eq!(l.available(&AccountId::User), Mtk::from_tokens(90));

    // Generation failed (import-reject / provider-error) → release; the tokens return to available.
    l.release(hold, "gen-1").expect("release an open hold");
    assert_eq!(
        l.balance(&AccountId::User),
        Mtk::from_tokens(100),
        "never charged for a failure"
    );
    assert_eq!(l.available(&AccountId::User), Mtk::from_tokens(100));
    assert_eq!(l.balance(&AccountId::Platform), Mtk::ZERO);

    // Releasing again, or settling a released hold, is a no-op (no resurrection).
    assert!(l.release(hold, "gen-1").is_none());
    assert!(l.settle(hold, "gen-1").is_none());
    assert_eq!(l.balance(&AccountId::User), Mtk::from_tokens(100));
}

#[test]
fn orphan_holds_are_swept_to_release_on_load() {
    let mut l = Ledger::new();
    l.top_up(AccountId::User, 100, "sb");
    let _h = l
        .reserve(&AccountId::User, &Action::Generate, "gen-1")
        .unwrap();
    assert_eq!(l.available(&AccountId::User), Mtk::from_tokens(90));
    // Simulate a crash mid-flight, then a startup sweep: the orphan hold is released (refunded).
    let swept = l.sweep_open_holds();
    assert_eq!(swept.len(), 1);
    assert_eq!(
        l.available(&AccountId::User),
        Mtk::from_tokens(100),
        "the in-flight hold was refunded"
    );
    assert!(l.sweep_open_holds().is_empty(), "nothing left to sweep");
}

#[test]
fn a_marketplace_buy_accrues_seventy_percent_to_the_creator_and_conserves_tokens() {
    let mut l = Ledger::new();
    l.top_up(AccountId::User, 100, "sb");
    let buy = Action::Buy {
        price_tokens: 4,
        creator: Some("forge".to_string()),
    };
    l.charge(&AccountId::User, &buy, "buy-forge-1").unwrap();

    assert_eq!(
        l.balance(&AccountId::User),
        Mtk::from_tokens(96),
        "user paid 4"
    );
    assert_eq!(
        l.balance(&AccountId::Creator("forge".to_string())),
        Mtk(2800),
        "creator accrues exactly 70% (2.8 tokens)"
    );
    assert_eq!(
        l.balance(&AccountId::Platform),
        Mtk(1200),
        "platform 30% (1.2 tokens)"
    );

    // Global conservation: Σ all balances == Σ grants (the top-up), nothing minted/burned.
    assert_eq!(
        total_across_accounts(&l, &["forge"]),
        Mtk::from_tokens(100).get(),
        "tokens are conserved across the buy"
    );
}

#[test]
fn an_unprefixed_entry_credits_the_platform_never_an_empty_creator() {
    let mut l = Ledger::new();
    l.top_up(AccountId::User, 100, "sb");
    let buy = Action::Buy {
        price_tokens: 3,
        creator: None, // an entry id with no namespace
    };
    l.charge(&AccountId::User, &buy, "buy-anon").unwrap();
    assert_eq!(l.balance(&AccountId::User), Mtk::from_tokens(97));
    assert_eq!(
        l.balance(&AccountId::Platform),
        Mtk::from_tokens(3),
        "whole price → platform"
    );
    assert_eq!(
        l.balance(&AccountId::Creator(String::new())),
        Mtk::ZERO,
        "never an empty-string creator"
    );
}

#[test]
fn a_free_priced_entry_never_touches_the_ledger() {
    let mut l = Ledger::new();
    l.top_up(AccountId::User, 100, "sb");
    let before = l.len();
    let buy = Action::Buy {
        price_tokens: 0,
        creator: Some("forge".to_string()),
    };
    let charged = l.charge(&AccountId::User, &buy, "buy-free").unwrap();
    assert_eq!(charged, Mtk::ZERO);
    assert_eq!(l.len(), before, "a free entry appends nothing");
}

#[test]
fn free_tier_is_granted_once_so_relaunch_cannot_farm_tokens() {
    let mut l = Ledger::new();
    assert!(l
        .grant_free_tier(AccountId::User, Mtk::from_tokens(FREE_GRANT_TOKENS))
        .is_some());
    // A "relaunch" tries to seed again → refused (idempotent), balance unchanged.
    assert!(l
        .grant_free_tier(AccountId::User, Mtk::from_tokens(FREE_GRANT_TOKENS))
        .is_none());
    assert_eq!(
        l.balance(&AccountId::User),
        Mtk::from_tokens(FREE_GRANT_TOKENS),
        "the free grant is not farmable by relaunching"
    );
}

#[test]
fn sandbox_top_up_grants_tokens_via_a_ledger_entry() {
    let mut l = Ledger::new();
    let provider = SandboxProvider;
    let receipt = provider.charge(1000, "topup-1").expect("sandbox charge");
    l.top_up(AccountId::User, receipt.tokens, &receipt.provider_ref);
    assert_eq!(
        l.balance(&AccountId::User),
        Mtk::from_tokens(100),
        "$10 ⇒ 100 tokens granted"
    );
    // The grant is an ordinary auditable ledger entry.
    assert_eq!(l.entries().len(), 1);
}

#[test]
fn buy_plus_edit_costs_less_than_a_regenerate_end_to_end() {
    // Encode the ADR-004 guarantee as a spend comparison, not just a cost-model unit test.
    let buy = Action::Buy {
        price_tokens: 4,
        creator: Some("forge".to_string()),
    };
    let buy_plus_edit = cost(&buy).get() + cost(&Action::Edit).get();
    let regenerate = cost(&Action::Generate).get();
    assert!(
        buy_plus_edit < regenerate,
        "buy(4)+edit(2)={buy_plus_edit}mt must be < regenerate={regenerate}mt"
    );
}
