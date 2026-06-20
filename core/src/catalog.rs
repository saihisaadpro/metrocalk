//! The unified "+ Add" catalog query (M3.4) — **one source** over the registry (stdlib kinds) + the
//! [`MarketplaceIndex`] (M5), grouped by category, and searched by **reusing the resolver** (no parallel
//! search path). Pure metadata → wasm-portable (ADR-006). The shell renders it; choosing an item
//! instantiates through the one pipeline (the same path as describe-to-create), so Add and describe
//! converge.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::caps::display_name;
use crate::marketplace::{MarketplaceEntry, MarketplaceIndex};
use crate::registry::ComponentMeta;
use crate::resolve::Resolved;
use crate::taxonomy::bucket_of;

/// Where a catalog item comes from (the tier its Add instantiates through).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// A stdlib component kind — a free local instantiate.
    Local,
    /// A marketplace entry — a metered buy (M7) + apply.
    Marketplace,
}

/// One browsable catalog item — a pre-componentized working object, previewed by the caps it brings +
/// its mesh (not a bare name). `id` = the kind name (local) or the entry id (marketplace) — the handle
/// the Add action instantiates.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogItem {
    /// Kind name (local) or entry id (marketplace) — what Add instantiates.
    pub id: String,
    /// Display label.
    pub label: String,
    /// The standard category bucket (canonical, for grouping).
    pub bucket: String,
    /// A short display category (`UI`, `Characters (acme)`).
    pub category: String,
    /// Which tier this comes from.
    pub source: Source,
    /// Provided capabilities (display names — the preview of what it brings).
    pub provides: Vec<String>,
    /// Required capabilities (what it can attach to).
    pub requires: Vec<String>,
    /// The logical mesh/asset name, if any (preview).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// Token price (marketplace only; `None` = free local).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<u32>,
    /// Search rank in `[0,1]` (`None` outside a search).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

fn local_item(m: &ComponentMeta, score: Option<f32>) -> CatalogItem {
    let bucket = bucket_of(m.category.as_deref());
    CatalogItem {
        id: m.name.clone(),
        label: m.name.clone(),
        category: display_name(&bucket),
        bucket,
        source: Source::Local,
        provides: m.provides.iter().map(|c| display_name(c)).collect(),
        requires: m.requires.iter().map(|c| display_name(c)).collect(),
        asset: None,
        price: None,
        score,
    }
}

fn market_item(e: &MarketplaceEntry, score: Option<f32>) -> CatalogItem {
    CatalogItem {
        id: e.id.clone(),
        label: e.name.clone(),
        bucket: e.category.bucket(),
        category: e.category.display(),
        source: Source::Marketplace,
        provides: e.provides.iter().map(|c| display_name(&c.name)).collect(),
        requires: e.requires.iter().map(|c| display_name(&c.name)).collect(),
        asset: e.asset.clone(),
        price: e.price,
        score,
    }
}

/// The whole catalog (stdlib kinds + every browsable marketplace entry), unranked.
#[must_use]
pub fn all<I: MarketplaceIndex>(metas: &[ComponentMeta], index: &I) -> Vec<CatalogItem> {
    let mut items: Vec<CatalogItem> = metas.iter().map(|m| local_item(m, None)).collect();
    items.extend(index.all().iter().map(|e| market_item(e, None)));
    items
}

/// The catalog grouped by **standard category bucket** (the browse view) — buckets keyed canonically,
/// items sorted by label, for a stable UI.
#[must_use]
pub fn grouped<I: MarketplaceIndex>(
    metas: &[ComponentMeta],
    index: &I,
) -> BTreeMap<String, Vec<CatalogItem>> {
    let mut by: BTreeMap<String, Vec<CatalogItem>> = BTreeMap::new();
    for item in all(metas, index) {
        by.entry(item.bucket.clone()).or_default().push(item);
    }
    for items in by.values_mut() {
        items.sort_by(|a, b| a.label.cmp(&b.label));
    }
    by
}

/// The seam a search escalates to when nothing local/marketplace matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchSeam {
    /// Nothing anywhere — offer the generate tier (the resolution order, made browsable).
    Generate,
}

/// A catalog search result — ranked items + (when empty) the seam to offer next.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSearch {
    /// Ranked matches (local first, else marketplace).
    pub items: Vec<CatalogItem>,
    /// The fall-through seam when nothing matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seam: Option<SearchSeam>,
}

/// Search the catalog by `query`, **reusing the tiered resolver** ([`crate::resolve::resolve`] over
/// `resolve_local` + the marketplace index) — one source, no parallel search path: local matches first
/// (ranked), else the marketplace tier, else the generate seam (a no-hit falls through honestly).
#[must_use]
pub fn search<I: MarketplaceIndex>(
    metas: &[ComponentMeta],
    index: &I,
    query: &str,
) -> CatalogSearch {
    match crate::resolve::resolve(metas, index, query) {
        Resolved::Local(matches) => CatalogSearch {
            items: matches
                .iter()
                .filter_map(|mat| {
                    metas
                        .iter()
                        .find(|m| m.name == mat.kind)
                        .map(|m| local_item(m, Some(mat.score)))
                })
                .collect(),
            seam: None,
        },
        Resolved::Marketplace(matches) => CatalogSearch {
            items: matches
                .iter()
                .map(|mm| market_item(&mm.entry, Some(mm.score)))
                .collect(),
            seam: None,
        },
        Resolved::Generate => CatalogSearch {
            items: Vec::new(),
            seam: Some(SearchSeam::Generate),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marketplace::LocalCatalog;
    use crate::stdlib::standard_components;

    #[test]
    fn the_catalog_is_one_source_over_registry_plus_marketplace() {
        let metas = standard_components();
        let cat = LocalCatalog::builtin();
        let items = all(&metas, &cat);
        assert_eq!(
            items.len(),
            metas.len() + cat.len(),
            "every stdlib kind + every marketplace entry, one source"
        );
        assert!(items
            .iter()
            .any(|i| i.source == Source::Local && i.id == "HealthBar"));
        assert!(items
            .iter()
            .any(|i| i.source == Source::Marketplace && i.id == "forge:rusty-sword"));
    }

    #[test]
    fn grouped_buckets_marketplace_custom_categories_under_their_std_alias() {
        let metas = standard_components();
        let g = grouped(&metas, &LocalCatalog::builtin());
        // Both acme + brandx companions alias to std:Characters → they browse together.
        let characters = g.get("std:Characters").expect("a Characters bucket");
        assert!(characters.iter().any(|i| i.id == "acme:companion-drone"));
        assert!(characters.iter().any(|i| i.id == "brandx:spirit-familiar"));
        // The sword is Props.
        assert!(g
            .get("std:Props")
            .unwrap()
            .iter()
            .any(|i| i.id == "forge:rusty-sword"));
    }

    #[test]
    fn search_reuses_the_resolver_local_then_marketplace_then_generate() {
        let metas = standard_components();
        let cat = LocalCatalog::builtin();
        // Local hit (ranked, scored).
        let r = search(&metas, &cat, "health bar");
        assert!(
            r.seam.is_none() && r.items[0].source == Source::Local && r.items[0].score.is_some()
        );
        // No local → marketplace.
        let r = search(&metas, &cat, "rusty medieval sword");
        assert!(r.seam.is_none() && r.items.iter().all(|i| i.source == Source::Marketplace));
        assert_eq!(r.items[0].id, "forge:rusty-sword");
        // Nothing anywhere → the honest generate seam, no items.
        let r = search(&metas, &cat, "zzqxw plumbus nonsense");
        assert!(r.items.is_empty() && r.seam == Some(SearchSeam::Generate));
    }
}
