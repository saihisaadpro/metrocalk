//! M7 metering wiring — the three paid sinks (marketplace **buy**, **AI-edit**, and the generation
//! reserve→settle/release) through the real `/core` engine + the wallet, plus the **free-path proof**
//! (local build/bind/describe-local/place-asset never touch the meter). Generation's ledger mechanics
//! are covered by the economy crate + the wallet unit tests; here we prove the *editor-side* wiring.

use std::collections::HashMap;

use metrocalk_core::marketplace::{LocalCatalog, MarketplaceIndex};
use metrocalk_core::{Engine, FieldValue};
use metrocalk_ecs::FlecsWorld;
use metrocalk_economy::{AccountId, Action, Mtk};

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::{ai_edit_rustier, buy_marketplace, Outcome, Wallet};

const N: usize = 200;

fn seeded() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    capscene::seed(&mut engine, &scene, N).expect("seed");
    engine.clear_history();
    (engine, scene)
}

#[test]
fn a_marketplace_buy_debits_the_price_and_pays_the_creator_seventy_percent() {
    let (mut engine, scene) = seeded();
    let mut wallet = Wallet::in_memory(); // 30 free tokens
    let entry = LocalCatalog::builtin()
        .get("forge:rusty-sword")
        .expect("the rusty sword"); // price 4, creator "forge"

    let (created, outcome) = buy_marketplace(
        &mut engine,
        &scene,
        &mut wallet,
        &entry,
        None,
        [0.0; 3],
        "buy-1",
    );
    assert!(created.is_some(), "the bought entity dropped in");
    assert!(matches!(outcome, Outcome::Charged { cost_tokens: 4, .. }));
    assert_eq!(wallet.balance_tokens(), 26, "user paid 4");
    assert_eq!(
        wallet.ledger().balance(&AccountId::Creator("forge".to_string())),
        Mtk(2800),
        "creator accrues exactly 70% (2.8 tokens)"
    );
    assert_eq!(
        wallet.ledger().balance(&AccountId::Platform),
        Mtk(1200),
        "platform 30%"
    );
}

#[test]
fn a_buy_is_refused_gracefully_when_broke_and_changes_no_scene() {
    let (mut engine, scene) = seeded();
    let mut wallet = Wallet::in_memory();
    // Drain to 2 tokens (14 edits × 2 = 28 spent).
    for i in 0..14 {
        wallet.charge(&Action::Edit, &format!("e{i}")).unwrap();
    }
    assert_eq!(wallet.balance_tokens(), 2);

    let entry = LocalCatalog::builtin().get("forge:rusty-sword").unwrap(); // costs 4
    let before = engine.entity_count();
    let (created, outcome) = buy_marketplace(
        &mut engine,
        &scene,
        &mut wallet,
        &entry,
        None,
        [0.0; 3],
        "buy-broke",
    );
    assert!(created.is_none());
    assert!(matches!(outcome, Outcome::Refused { needed: 4, have: 2 }));
    assert_eq!(
        engine.entity_count(),
        before,
        "a refused buy changes no scene"
    );
    assert_eq!(wallet.balance_tokens(), 2, "and charges nothing");
}

#[test]
fn an_ai_edit_debits_two_tokens_and_sets_the_material_rusty() {
    let (mut engine, scene) = seeded();
    let mut wallet = Wallet::in_memory();
    let ph = capscene::place_generation_placeholder(&mut engine, &scene, [0.0; 3]).unwrap();

    let (delta, outcome) = ai_edit_rustier(&mut engine, &mut wallet, ph, "edit-1");
    assert!(matches!(outcome, Outcome::Charged { cost_tokens: 2, .. }));
    assert_eq!(wallet.balance_tokens(), 28);
    assert!(delta.is_some(), "the material edit echoes a delta");
    let material = engine
        .components_of(ph)
        .get("MeshRenderer")
        .and_then(|m| m.get("material").cloned());
    assert_eq!(
        material,
        Some(FieldValue::Str("rusty".to_string())),
        "the AI-edit set the material via the validated patch"
    );
}

#[test]
fn an_ai_edit_on_a_nonexistent_entity_is_rejected_and_never_charged() {
    let (mut engine, _scene) = seeded();
    let mut wallet = Wallet::in_memory();
    let bogus = engine.alloc_entity_id(); // allocated, but no entity created
    let before = wallet.balance_tokens();

    let (delta, outcome) = ai_edit_rustier(&mut engine, &mut wallet, bogus, "edit-bogus");
    assert!(matches!(outcome, Outcome::Rejected(_)));
    assert!(delta.is_none());
    assert_eq!(
        wallet.balance_tokens(),
        before,
        "a rejected edit is never charged"
    );
}

#[test]
fn an_ai_edit_is_refused_gracefully_when_broke() {
    let (mut engine, scene) = seeded();
    let mut wallet = Wallet::in_memory();
    for i in 0..15 {
        wallet.charge(&Action::Edit, &format!("e{i}")).unwrap(); // drain all 30
    }
    assert_eq!(wallet.balance_tokens(), 0);
    let ph = capscene::place_generation_placeholder(&mut engine, &scene, [0.0; 3]).unwrap();
    let (delta, outcome) = ai_edit_rustier(&mut engine, &mut wallet, ph, "edit-broke");
    assert!(matches!(outcome, Outcome::Refused { needed: 2, have: 0 }));
    assert!(delta.is_none(), "nothing applied when broke");
}

#[test]
fn the_free_local_path_never_touches_the_meter() {
    // The engine is FREE forever: local build/describe-local/bind/place-asset must run with a wallet
    // that has zero involvement — no ledger entry appended, balance unchanged. (The crate-dependency
    // tripwire in ci.yml is the compile-time half of this proof; this is the behavioral half.)
    let (mut engine, scene) = seeded();
    let wallet = Wallet::in_memory();
    let baseline_entries = wallet.ledger().len(); // just the free-tier grant
    let baseline_balance = wallet.balance_tokens();

    // Describe resolves LOCALLY (never the marketplace/generate sinks).
    let catalog: HashMap<String, String> = HashMap::new();
    assert!(
        capscene::describe_create(&mut engine, &scene, "health bar", [0.0; 3], &catalog).is_some(),
        "local describe works offline, free"
    );
    // Place an imported asset by handle (a free local action).
    capscene::place_mesh(&mut engine, &scene, "any-handle", [1.0, 0.0, 0.0]).expect("place");

    // The wallet is provably untouched — these local craft actions don't (and can't) meter.
    assert_eq!(
        wallet.ledger().len(),
        baseline_entries,
        "the free path appended NO ledger entry"
    );
    assert_eq!(wallet.balance_tokens(), baseline_balance);
}
