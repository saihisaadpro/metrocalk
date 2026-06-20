//! Category taxonomy for the "+ Add" catalog browser (M3.4) — mirrors capability identity (ADR-015): a
//! curated `std:` **standard vocabulary** + author-namespaced custom categories that opt into a standard
//! **bucket** via an alias, so two authors' custom categories never fragment the browsable taxonomy.
//!
//! Pure naming — it reuses [`crate::caps`]'s `ns:name` rule and adds the curated standard set + the
//! covariant "groups-under" bucket resolution. No ECS/Loro → wasm-portable like the resolver (ADR-006).

use serde::{Deserialize, Serialize};

use crate::caps::{canonical, display_name, local_name, namespace, STD};

/// The curated **standard** category vocabulary — the browsable buckets, in display order.
pub const STD_CATEGORIES: &[&str] = &["UI", "Gameplay", "Props", "Characters", "Audio", "Logic"];

/// The catch-all bucket for an uncategorized item or a custom category with no standard alias.
pub const OTHER: &str = "std:Other";

/// Whether `name` (canonicalized) is one of the curated standard categories.
#[must_use]
pub fn is_standard_category(name: &str) -> bool {
    let c = canonical(name);
    namespace(&c) == STD && STD_CATEGORIES.contains(&local_name(&c))
}

/// A category an item belongs to — a namespaced name + an optional alias to a standard bucket (mirrors
/// [`crate::marketplace::CapDecl`]). A bare/`std:` standard name groups under itself; a custom
/// `acme:Vehicles` opts into a standard bucket (e.g. `std:Props`) so it browses there without
/// fragmenting the taxonomy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    /// The category name (namespaced, e.g. `std:UI` or `acme:Vehicles`).
    pub name: String,
    /// The standard bucket this opts into, if custom (e.g. `std:Props`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias_of: Option<String>,
}

impl Category {
    /// A standard-vocabulary category (`Props` → `std:Props`).
    #[must_use]
    pub fn std(name: &str) -> Self {
        Self {
            name: canonical(name),
            alias_of: None,
        }
    }
    /// A namespaced custom category that opts into a standard `bucket` via an alias.
    #[must_use]
    pub fn aliased(name: &str, bucket: &str) -> Self {
        Self {
            name: name.to_string(),
            alias_of: Some(canonical(bucket)),
        }
    }
    /// A namespaced custom category with no standard alias (browses under [`OTHER`]).
    #[must_use]
    pub fn custom(name: &str) -> Self {
        Self {
            name: name.to_string(),
            alias_of: None,
        }
    }

    /// The **standard browsable bucket** this category groups under: its alias target if custom, else
    /// itself if it's a standard category, else [`OTHER`]. Two authors' custom categories that alias the
    /// same standard bucket browse **together**; an un-aliased custom category never collides with a
    /// standard bucket.
    #[must_use]
    pub fn bucket(&self) -> String {
        // Honor an alias ONLY when it points to a STANDARD bucket — a malformed/reverse alias to a
        // non-standard category falls to `Other`, so a bad entry can never fragment a standard bucket
        // (the taxonomy invariant holds even for an untrusted marketplace provider's metadata).
        if let Some(a) = &self.alias_of {
            let c = canonical(a);
            return if is_standard_category(&c) {
                c
            } else {
                OTHER.to_string()
            };
        }
        let c = canonical(&self.name);
        if is_standard_category(&c) {
            c
        } else {
            OTHER.to_string()
        }
    }

    /// A short human label (`std:UI` → `UI`; `acme:Vehicles` → `Vehicles (acme)`).
    #[must_use]
    pub fn display(&self) -> String {
        display_name(&self.name)
    }
}

/// The standard bucket for an optional canonical category string (a [`crate::registry::ComponentMeta`]'s
/// `category`): the category itself if standard, else [`OTHER`]. (Stdlib kinds carry a plain canonical
/// category; only marketplace entries carry the alias-bearing [`Category`].)
#[must_use]
pub fn bucket_of(category: Option<&str>) -> String {
    match category {
        Some(c) if is_standard_category(c) => canonical(c),
        _ => OTHER.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_categories_bucket_under_themselves() {
        assert_eq!(Category::std("Props").bucket(), "std:Props");
        assert_eq!(Category::std("UI").bucket(), "std:UI");
        assert!(is_standard_category("Props"));
        assert!(is_standard_category("std:UI"));
        assert!(!is_standard_category("acme:Vehicles"));
    }

    #[test]
    fn two_authors_custom_categories_alias_to_the_same_bucket_without_colliding() {
        let acme = Category::aliased("acme:Companions", "Characters");
        let brandx = Category::aliased("brandx:Familiars", "Characters");
        // Distinct identities…
        assert_ne!(acme.name, brandx.name);
        // …but they browse under the SAME standard bucket (no taxonomy fragmentation).
        assert_eq!(acme.bucket(), "std:Characters");
        assert_eq!(brandx.bucket(), "std:Characters");
        assert_eq!(acme.bucket(), brandx.bucket());
    }

    #[test]
    fn an_unaliased_custom_category_falls_to_other_never_a_std_bucket() {
        let c = Category::custom("acme:Vehicles");
        assert_eq!(
            c.bucket(),
            OTHER,
            "no bare-string fragmenting of a std bucket"
        );
        assert_eq!(c.display(), "Vehicles (acme)");
    }

    #[test]
    fn a_malformed_alias_to_a_non_standard_bucket_falls_to_other_never_fragments() {
        // An untrusted entry whose alias points to a NON-standard category must not create a parallel
        // bucket — it falls to Other (the invariant holds against bad provider metadata).
        assert_eq!(
            Category::aliased("acme:X", "acme:NotAStandardBucket").bucket(),
            OTHER
        );
        assert_eq!(
            Category::aliased("std:Props", "brandx:Junk").bucket(),
            OTHER
        );
    }

    #[test]
    fn uncategorized_kinds_bucket_under_other() {
        assert_eq!(bucket_of(None), OTHER);
        assert_eq!(bucket_of(Some("UI")), "std:UI");
        assert_eq!(bucket_of(Some("acme:Vehicles")), OTHER);
    }
}
