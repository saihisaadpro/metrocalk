//! Describe-to-create local resolver (M3.2, north-star test #2). Verifies offline semantic-ish
//! resolution over the curated stdlib: known descriptions resolve to the right kind carrying its real
//! capabilities; the adversarial cases (confident nonsense / no local asset) honestly return no match
//! and fall through to the marketplace seam — a wrong match is never returned.

#![allow(clippy::cast_precision_loss)]

use metrocalk_core::resolve::{resolve_local, NextTier};
use metrocalk_core::stdlib::standard_components;

fn top(query: &str) -> Option<String> {
    let lib = standard_components();
    resolve_local(&lib, query)
        .matches
        .first()
        .map(|m| m.kind.clone())
}

#[test]
fn resolves_descriptions_to_the_right_stdlib_kind() {
    assert_eq!(top("health bar"), Some("HealthBar".to_string()));
    assert_eq!(top("hp bar"), Some("HealthBar".to_string())); // alias
    assert_eq!(top("a light"), Some("Light".to_string()));
    assert_eq!(top("sound"), Some("AudioSource".to_string())); // synonym → audio
    assert_eq!(top("camera"), Some("Camera".to_string()));
    assert_eq!(top("rigid body"), Some("RigidBody".to_string()));
}

#[test]
fn a_match_carries_real_capabilities_not_dead_geometry() {
    let lib = standard_components();
    let r = resolve_local(&lib, "health bar");
    let m = &r.matches[0];
    assert_eq!(m.kind, "HealthBar");
    // The HealthBar kind requires Health + provides UIElement — a working object, not dead geometry.
    assert!(
        m.requires.contains(&"Health".to_string()),
        "HealthBar requires Health"
    );
    assert!(
        m.provides.contains(&"UIElement".to_string()),
        "HealthBar provides UIElement"
    );
    assert!(
        r.next_tier.is_none(),
        "a confident local match does not escalate"
    );
}

#[test]
fn confident_nonsense_and_missing_assets_honestly_return_no_match() {
    let lib = standard_components();
    // "rusty medieval sword": no local asset (no Sword kind) → honest no-match → marketplace seam,
    // NOT a wrong match (the adversarial guard).
    let sword = resolve_local(&lib, "rusty medieval sword");
    assert!(sword.matches.is_empty(), "no local sword → no match");
    assert_eq!(sword.next_tier, Some(NextTier::Marketplace));

    // Pure gibberish likewise.
    let nonsense = resolve_local(&lib, "zzqqx wibble");
    assert!(nonsense.matches.is_empty());
    assert_eq!(nonsense.next_tier, Some(NextTier::Marketplace));

    // Empty query → no match, seam.
    assert!(resolve_local(&lib, "").matches.is_empty());
}

#[test]
fn ranking_is_deterministic() {
    let lib = standard_components();
    let a: Vec<String> = resolve_local(&lib, "render")
        .matches
        .iter()
        .map(|m| m.kind.clone())
        .collect();
    let b: Vec<String> = resolve_local(&lib, "render")
        .matches
        .iter()
        .map(|m| m.kind.clone())
        .collect();
    assert_eq!(a, b, "same query → identical ranked order");
}
