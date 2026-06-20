//! Capability identity for an open ecosystem (the marketplace gate, ADR-015).
//!
//! The M1.3 registry interns capabilities by **bare string**, so two authors' `"Health"` collide —
//! fine for the curated stdlib, wrong for an open marketplace + describe-to-create. The fix is a
//! **standard vocabulary** (canonical stdlib caps, `std:Health`) + **author/package-namespaced** custom
//! caps (`acme:Shield`) that opt into the standard relational web via an `(AliasOf, std:Cap)` pair.
//!
//! This module is the pure naming layer: one canonicalization rule that makes the curated stdlib and a
//! marketplace entry meet at the **same** `std:*` cap while two authors' custom caps stay **distinct**.
//! No ECS, no Loro — wasm-portable by construction (ADR-006), like the resolver. The relational
//! machinery (interning a cap entity, recording the `(AliasOf, …)` pair, resolving an alias to its
//! standard cap at instantiate) lives where the caps become ECS entities (`editor-shell::capscene`);
//! the **rule** that keeps it coherent is here.

/// The standard-vocabulary namespace — the curated stdlib caps live here.
pub const STD: &str = "std";

/// The namespace separator.
const SEP: char = ':';

/// Canonicalize a capability name. A **bare** name (no namespace) is the standard vocabulary, so
/// `Health` → `std:Health`; an already-namespaced name (`std:Health`, `acme:Shield`) is returned
/// unchanged. This single rule is why the curated stdlib's bare `"Health"` and a marketplace entry's
/// `"std:Health"` intern to the *same* entity, while `acme:Health` and `brandx:Health` stay distinct.
#[must_use]
pub fn canonical(name: &str) -> String {
    if name.contains(SEP) {
        name.to_string()
    } else {
        format!("{STD}{SEP}{name}")
    }
}

/// The namespace of a capability name (`acme:Shield` → `acme`; a bare name → `std`).
#[must_use]
pub fn namespace(name: &str) -> &str {
    name.split_once(SEP).map_or(STD, |(ns, _)| ns)
}

/// The local (un-namespaced) part of a capability name (`acme:Shield` → `Shield`; a bare name as-is).
#[must_use]
pub fn local_name(name: &str) -> &str {
    name.split_once(SEP).map_or(name, |(_, n)| n)
}

/// Whether a capability is part of the standard vocabulary (bare or `std:`-prefixed).
#[must_use]
pub fn is_standard(name: &str) -> bool {
    namespace(name) == STD
}

/// A short human label for a capability — its local name, suffixed with the author namespace when it
/// isn't standard (so the UI reads `Shield (acme)` rather than the raw `acme:Shield`).
#[must_use]
pub fn display_name(name: &str) -> String {
    if is_standard(name) {
        local_name(name).to_string()
    } else {
        format!("{} ({})", local_name(name), namespace(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_stdlib_name_canonicalizes_into_std() {
        assert_eq!(canonical("Health"), "std:Health");
        assert_eq!(canonical("Renderable"), "std:Renderable");
    }

    #[test]
    fn already_namespaced_names_are_unchanged() {
        assert_eq!(canonical("std:Health"), "std:Health");
        assert_eq!(canonical("acme:Shield"), "acme:Shield");
    }

    #[test]
    fn bare_and_std_prefixed_collapse_to_the_same_key() {
        // The whole point: the curated stdlib (bare) and a marketplace entry (std:) meet at one entity.
        assert_eq!(canonical("Health"), canonical("std:Health"));
    }

    #[test]
    fn two_authors_same_local_name_stay_distinct() {
        // The collision the bare-string registry would have had is impossible now.
        assert_ne!(canonical("acme:Health"), canonical("brandx:Health"));
        assert_ne!(canonical("acme:Health"), canonical("Health"));
    }

    #[test]
    fn namespace_and_local_parts() {
        assert_eq!(namespace("acme:Shield"), "acme");
        assert_eq!(namespace("Health"), "std");
        assert_eq!(namespace("std:Health"), "std");
        assert_eq!(local_name("acme:Shield"), "Shield");
        assert_eq!(local_name("Health"), "Health");
    }

    #[test]
    fn standard_vocabulary_predicate() {
        assert!(is_standard("Health"));
        assert!(is_standard("std:Health"));
        assert!(!is_standard("acme:Shield"));
    }

    #[test]
    fn display_names_are_readable() {
        assert_eq!(display_name("std:Health"), "Health");
        assert_eq!(display_name("Health"), "Health");
        assert_eq!(display_name("acme:Shield"), "Shield (acme)");
    }
}
