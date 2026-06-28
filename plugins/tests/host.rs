//! M12.3 (ADR-047) — the WASM plugin host, headless. The fixture is the REAL example plugin
//! (`example-plugin/`, built to wasm32 via extism-pdk; checked in at `fixtures/arrange.wasm`). Covers the
//! DoD: a plugin loads + runs **behind the `PluginHost` trait** (this test uses ONLY our public types — no
//! `extism::` — which is also CI-grep-gated); it's **deterministic** (Play/replay eligible); and a
//! misbehaving plugin (missing export · malformed input · a starved budget) is **contained + explained**,
//! never a host crash. The effect-is-a-transaction + undoable round-trip is in the editor-shell tests
//! (the host is pure bytes-in/bytes-out; the commit pipeline lives there).

use std::time::Duration;

use metrocalk_plugins::{ExtismHost, PluginError, PluginHost, PluginInstance, Sandbox};

const ARRANGE_WASM: &[u8] = include_bytes!("fixtures/arrange.wasm");

fn host() -> ExtismHost {
    ExtismHost::new()
}

#[test]
fn a_plugin_loads_and_runs_behind_the_trait() {
    let mut p = host()
        .load(ARRANGE_WASM, &Sandbox::restrictive())
        .expect("the example plugin loads behind the trait");
    let input = br#"{"ids":["1_0","1_1","1_2"],"seed":7,"spacing":2.0}"#;
    let out = p.call("arrange", input).expect("the plugin runs");
    // The output is an AiPatch (the ADR-017 shape the host commits) — 3 entities × (px,py,pz) setField ops.
    let patch: serde_json::Value = serde_json::from_slice(&out).expect("output is JSON");
    assert_eq!(patch["clientOpId"], "plugin-arrange");
    let ops = patch["ops"].as_array().expect("ops array");
    assert_eq!(ops.len(), 9);
    assert_eq!(ops[0]["op"], "setField");
    assert_eq!(ops[0]["component"], "Transform");
    assert_eq!(ops[0]["id"], "1_0");
    assert!(
        ops[0]["value"].is_number(),
        "Transform.px is a numeric value"
    );
}

#[test]
fn the_plugin_is_deterministic_play_replay_eligible() {
    let input = br#"{"ids":["1_0","1_1","1_2","1_3"],"seed":42,"spacing":1.5}"#;
    let mut p1 = host().load(ARRANGE_WASM, &Sandbox::restrictive()).unwrap();
    let mut p2 = host().load(ARRANGE_WASM, &Sandbox::restrictive()).unwrap();
    let a = p1.call("arrange", input).unwrap();
    let b = p2.call("arrange", input).unwrap();
    assert_eq!(
        a, b,
        "same plugin + input → byte-identical output (the determinism the Play/replay path needs)"
    );
}

#[test]
fn a_missing_export_is_contained_not_a_crash() {
    let mut p = host().load(ARRANGE_WASM, &Sandbox::restrictive()).unwrap();
    // Calling a function the plugin doesn't export returns an Err (contained) — never a panic/host crash.
    let err = p.call("does_not_exist", b"{}").unwrap_err();
    assert!(
        !err.to_string().is_empty(),
        "the failure explains itself: {err}"
    );
}

#[test]
fn malformed_input_is_contained_and_explained() {
    let mut p = host().load(ARRANGE_WASM, &Sandbox::restrictive()).unwrap();
    // The plugin rejects bad input with a non-zero return code → surfaced as a contained PluginError.
    let err = p.call("arrange", b"not json at all").unwrap_err();
    assert!(
        !err.to_string().is_empty(),
        "a malformed call is contained + explained, not a crash: {err}"
    );
}

#[test]
fn the_host_fn_allow_list_is_fail_closed() {
    // The allow-list is the capability boundary: a plugin gets ONLY granted host fns. Granting a capability
    // the host doesn't define is refused (fail-closed) — a sandbox can't hand out ambient access by typo.
    let sandbox = Sandbox {
        allowed_host_fns: vec!["network".to_string()],
        ..Sandbox::restrictive()
    };
    // (the loaded-plugin type isn't Debug, so let-else rather than unwrap_err)
    let Err(err) = host().load(ARRANGE_WASM, &sandbox) else {
        panic!("granting an unknown capability must be refused");
    };
    assert!(
        matches!(err, PluginError::DisallowedHostFn(_)),
        "an un-defined capability grant is refused (got {err:?})"
    );
    // The pure example plugin loads fine under the default (empty) allow-list — no ambient access needed.
    assert!(host().load(ARRANGE_WASM, &Sandbox::restrictive()).is_ok());
}

#[test]
fn a_starved_budget_contains_a_runaway() {
    // A tiny fuel budget can't finish arranging 200 entities → the plugin is interrupted (contained),
    // not a hang or a host crash. Fuel is deterministic, so this containment is reproducible.
    let sandbox = Sandbox {
        fuel_limit: Some(10_000),
        timeout: Duration::from_millis(250),
        ..Sandbox::restrictive()
    };
    let mut p = host().load(ARRANGE_WASM, &sandbox).unwrap();
    let ids: Vec<String> = (0..200).map(|i| format!("1_{i:x}")).collect();
    let input =
        serde_json::to_vec(&serde_json::json!({ "ids": ids, "seed": 1, "spacing": 1.0 })).unwrap();
    let err = p.call("arrange", &input).unwrap_err();
    assert!(
        matches!(
            err,
            PluginError::BudgetExceeded(_) | PluginError::Timeout(_) | PluginError::Call { .. }
        ),
        "a starved budget contains the runaway (got {err:?})"
    );
}
