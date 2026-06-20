//! The marketplace tier of describe-to-create (the marketplace gate, ADR-015 + ADR-012).
//!
//! M3.2 made resolution **local-only**; the marketplace + generate tiers were honest stubs. This is
//! the **marketplace tier**: a queryable index of **pre-componentized** entries — working objects
//! (a component + **namespaced** capability pairs + a prompt-23 mesh asset), not dead files — so a
//! description with no local match resolves to a result that drops in **already wired**.
//!
//! Behind the [`MarketplaceIndex`] trait (invariant 5): a [`LocalCatalog`] over a checked-in dataset
//! here, a remote index later, with no change to the caller. Pure metadata (no ECS/Loro) → wasm-
//! portable by construction (ADR-006), like the resolver; the ECS apply lives in `editor-shell`.
//! The **economy is seamed**: an entry may carry a token `price`, but no money moves (ADR-004).

use crate::caps::canonical;
use crate::resolve::{normalize, split_name};
use crate::taxonomy::Category;

/// A capability an entry provides or requires — **namespaced**, with an optional **one-directional**
/// alias to a standard cap. `acme:Health` aliased to `std:Health` means "my Health IS-A std Health":
/// it satisfies a `std:Health` requirer (covariant), but a `std:Health` provider does not
/// automatically satisfy an `acme:Health` requirer. Aliasing is opt-in, toward the standard vocab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapDecl {
    /// The capability name (namespaced, e.g. `acme:Health`, or a bare standard name).
    pub name: String,
    /// The standard cap this opts into, if any (e.g. `std:Health`).
    pub alias_of: Option<String>,
}

impl CapDecl {
    /// A standard-vocabulary capability (no alias needed — it *is* standard).
    #[must_use]
    pub fn std(name: &str) -> Self {
        Self {
            name: canonical(name),
            alias_of: None,
        }
    }

    /// A namespaced custom capability that opts into a standard cap via an alias.
    #[must_use]
    pub fn aliased(name: &str, alias_of: &str) -> Self {
        Self {
            name: name.to_string(),
            alias_of: Some(canonical(alias_of)),
        }
    }

    /// A namespaced custom capability with no standard alias (only same-namespace binds).
    #[must_use]
    pub fn custom(name: &str) -> Self {
        Self {
            name: name.to_string(),
            alias_of: None,
        }
    }

    /// The canonical cap name.
    #[must_use]
    pub fn canonical_name(&self) -> String {
        canonical(&self.name)
    }

    /// The canonical alias target, if any.
    #[must_use]
    pub fn canonical_alias(&self) -> Option<String> {
        self.alias_of.as_deref().map(canonical)
    }
}

/// A pre-componentized marketplace entry — the working object a description resolves to.
#[derive(Clone, Debug, PartialEq)]
pub struct MarketplaceEntry {
    /// Stable id (namespaced, e.g. `forge:rusty-sword`) — the replay key.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Free-text description (the describe-to-create match target).
    pub description: String,
    /// Search tags.
    pub tags: Vec<String>,
    /// The component kind attached on apply (inspector/display label).
    pub component: String,
    /// Capabilities the object provides (namespaced; aliases opt into the standard web).
    pub provides: Vec<CapDecl>,
    /// Capabilities it requires (drives the M3.1 reveal for one-click attach).
    pub requires: Vec<CapDecl>,
    /// Logical asset name (resolved to a content-addressed prompt-23 handle by the shell), if any.
    pub asset: Option<String>,
    /// Token price (ADR-004; charged via the M7 wallet on buy).
    pub price: Option<u32>,
    /// The catalog category this entry browses under (M3.4) — namespaced + optionally aliased to a
    /// standard bucket so two authors' custom categories never fragment the taxonomy (ADR-015 pattern).
    pub category: Category,
}

/// One ranked marketplace hit.
#[derive(Clone, Debug, PartialEq)]
pub struct MarketplaceMatch {
    /// The matched entry.
    pub entry: MarketplaceEntry,
    /// Fit score in `[0,1]`.
    pub score: f32,
}

/// The marketplace index (invariant 5) — query by description → ranked pre-componentized entries; a
/// remote impl implements this unchanged. No foreign types cross it.
pub trait MarketplaceIndex {
    /// Ranked entries matching `description` (best first, above the confidence gate; may be empty).
    fn query(&self, description: &str) -> Vec<MarketplaceMatch>;

    /// Fetch an entry by id — the deterministic replay path (re-apply a chosen entry after reload).
    fn get(&self, id: &str) -> Option<MarketplaceEntry>;

    /// All browsable entries (the "+ Add" catalog, M3.4). A remote impl returns its curated/paged
    /// browse set; the local catalog returns its whole dataset.
    fn all(&self) -> Vec<MarketplaceEntry>;
}

/// Same confidence gate as the resolver (ADR-012): a weak match is an honest no-match, not a wrong one.
const MIN_SCORE: f32 = 0.5;

/// A local, checked-in catalog — the mechanism a remote index will later implement unchanged.
pub struct LocalCatalog {
    entries: Vec<MarketplaceEntry>,
}

impl LocalCatalog {
    /// A catalog over an explicit entry list (tests / custom datasets).
    #[must_use]
    pub fn new(entries: Vec<MarketplaceEntry>) -> Self {
        Self { entries }
    }

    /// The checked-in built-in catalog.
    #[must_use]
    pub fn builtin() -> Self {
        Self::new(builtin_catalog())
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalog is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All entries (for interning their caps up front).
    #[must_use]
    pub fn entries(&self) -> &[MarketplaceEntry] {
        &self.entries
    }
}

impl MarketplaceIndex for LocalCatalog {
    fn query(&self, description: &str) -> Vec<MarketplaceMatch> {
        let q = normalize(description);
        if q.is_empty() {
            return Vec::new();
        }
        let mut matches: Vec<MarketplaceMatch> = self
            .entries
            .iter()
            .filter_map(|e| {
                let score = score_entry(e, &q);
                (score >= MIN_SCORE).then(|| MarketplaceMatch {
                    entry: e.clone(),
                    score,
                })
            })
            .collect();
        // Deterministic: score desc, then id asc.
        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.entry.id.cmp(&b.entry.id))
        });
        matches
    }

    fn get(&self, id: &str) -> Option<MarketplaceEntry> {
        self.entries.iter().find(|e| e.id == id).cloned()
    }

    fn all(&self) -> Vec<MarketplaceEntry> {
        self.entries.clone()
    }
}

/// Weighted token-overlap (same shape as the resolver's `score_meta`): the entry's display name +
/// tags + component (strong, 1.0) and its description + capability local-names (weak, 0.6), with a
/// substring fallback (0.3), normalized by query length. Capability namespaces are stripped before
/// tokenizing (so `acme:Shield` contributes `shield`).
#[allow(clippy::cast_precision_loss)]
fn score_entry(entry: &MarketplaceEntry, q: &[String]) -> f32 {
    let mut strong: Vec<String> = normalize(&entry.name);
    strong.extend(split_name(&entry.component));
    for t in &entry.tags {
        strong.extend(normalize(t));
    }
    let mut weak: Vec<String> = normalize(&entry.description);
    for c in entry.provides.iter().chain(entry.requires.iter()) {
        weak.extend(split_name(&c.name));
    }

    let mut total = 0.0f32;
    for qt in q {
        let w = if strong.iter().any(|s| s == qt) {
            1.0
        } else if weak.iter().any(|s| s == qt) {
            0.6
        } else if strong
            .iter()
            .chain(weak.iter())
            .any(|s| s.contains(qt) || qt.contains(s))
        {
            0.3
        } else {
            0.0
        };
        total += w;
    }
    total / q.len() as f32
}

/// The checked-in built-in marketplace catalog — a small set of pre-componentized entries that prove
/// the mechanism: a no-local-match resolves here, arriving already wired (namespaced caps + a mesh).
/// Two entries deliberately declare same-local-name custom caps (`acme:Health` / `brandx:Health`),
/// both aliased to `std:Health` — so they bind a `std:Health` requirer across authors yet never
/// collide with each other (the namespacing the open marketplace needs).
#[must_use]
pub fn builtin_catalog() -> Vec<MarketplaceEntry> {
    vec![
        MarketplaceEntry {
            id: "forge:rusty-sword".to_string(),
            name: "Rusty Medieval Sword".to_string(),
            description: "a rusty medieval sword you can pick up and swing".to_string(),
            tags: vec![
                "weapon".into(),
                "melee".into(),
                "pickup".into(),
                "sword".into(),
            ],
            component: "Weapon".to_string(),
            provides: vec![CapDecl::std("Renderable")],
            requires: vec![CapDecl::std("Spatial")],
            asset: Some("prop".to_string()),
            price: Some(4),
            category: Category::std("Props"),
        },
        MarketplaceEntry {
            id: "acme:companion-drone".to_string(),
            name: "Companion Drone".to_string(),
            description: "a hovering companion drone that shares its own health pool".to_string(),
            tags: vec!["companion".into(), "drone".into(), "ally".into()],
            component: "Companion".to_string(),
            // acme's own Health cap, opting into std:Health so a HealthBar can bind it.
            provides: vec![
                CapDecl::aliased("acme:Health", "Health"),
                CapDecl::std("Renderable"),
            ],
            requires: vec![CapDecl::std("Spatial")],
            asset: Some("prop".to_string()),
            price: Some(3),
            // A custom category that opts into the standard "Characters" bucket (ADR-015 pattern).
            category: Category::aliased("acme:Companions", "Characters"),
        },
        MarketplaceEntry {
            id: "brandx:spirit-familiar".to_string(),
            name: "Spirit Familiar".to_string(),
            description: "a glowing spirit familiar companion with its own health".to_string(),
            tags: vec!["familiar".into(), "spirit".into(), "companion".into()],
            component: "Familiar".to_string(),
            // brandx's Health cap — same local name as acme's, distinct namespace, also aliased to std.
            provides: vec![
                CapDecl::aliased("brandx:Health", "Health"),
                CapDecl::std("Renderable"),
            ],
            requires: vec![CapDecl::std("Spatial")],
            asset: Some("prop".to_string()),
            price: Some(2),
            // A DIFFERENT author's custom category, also aliasing "Characters" — they browse together,
            // never collide (the open-marketplace taxonomy guard).
            category: Category::aliased("brandx:Familiars", "Characters"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_local_phrasing_resolves_to_the_marketplace_entry() {
        let cat = LocalCatalog::builtin();
        let hits = cat.query("rusty medieval sword");
        assert!(!hits.is_empty(), "the sword resolves on the marketplace");
        assert_eq!(hits[0].entry.id, "forge:rusty-sword");
        assert!(hits[0].score >= MIN_SCORE);
    }

    #[test]
    fn companion_resolves_and_carries_namespaced_aliased_caps() {
        let cat = LocalCatalog::builtin();
        let hits = cat.query("companion");
        assert!(hits.iter().any(|m| m.entry.id == "acme:companion-drone"));
        let entry = cat.get("acme:companion-drone").unwrap();
        let health = entry
            .provides
            .iter()
            .find(|c| c.name == "acme:Health")
            .unwrap();
        assert_eq!(health.canonical_alias().as_deref(), Some("std:Health"));
    }

    #[test]
    fn two_authors_health_caps_are_distinct_but_both_alias_std() {
        let cat = LocalCatalog::builtin();
        let acme = cat.get("acme:companion-drone").unwrap();
        let brandx = cat.get("brandx:spirit-familiar").unwrap();
        let acme_h = acme
            .provides
            .iter()
            .find(|c| c.canonical_name() == "acme:Health")
            .unwrap();
        let brandx_h = brandx
            .provides
            .iter()
            .find(|c| c.canonical_name() == "brandx:Health")
            .unwrap();
        assert_ne!(
            acme_h.canonical_name(),
            brandx_h.canonical_name(),
            "distinct caps"
        );
        assert_eq!(
            acme_h.canonical_alias(),
            brandx_h.canonical_alias(),
            "both → std:Health"
        );
    }

    #[test]
    fn gibberish_is_an_honest_no_match() {
        let cat = LocalCatalog::builtin();
        assert!(
            cat.query("zzqxw plumbus").is_empty(),
            "no confident-but-wrong hit"
        );
        assert!(cat.query("").is_empty());
    }

    #[test]
    fn get_unknown_id_is_none() {
        assert!(LocalCatalog::builtin().get("nope:nothing").is_none());
    }
}
