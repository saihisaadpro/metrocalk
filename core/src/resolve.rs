//! Describe-to-create: the **local resolver** (north-star test #2, M3.2). Offline semantic-ish search
//! of the component metadata library by free-text description, tiered **local → marketplace →
//! generate** (the latter two are documented stubs — Phase-2 infra + the token economy).
//!
//! Approach (ADR-012): for a curated stdlib of ~12 kinds, the lightest thing that delivers genuinely
//! *semantic* matching offline is **token-overlap scoring** over each kind's camelCase-split name +
//! aliases + tags + provided/required capabilities, with a tiny synonym normalization — not a bundled
//! embedding model (overkill at this scale; revisit when the library/marketplace is large). A minimum
//! score gates "confident nonsense": an honest **no local match** (→ the marketplace/generate seam)
//! beats a wrong match. Pure metadata search — no ECS/Loro — so it runs identically native + wasm.

// The score is normalized by a small token count; f32 precision is irrelevant for a 0..1 ranking key.
#![allow(clippy::cast_precision_loss)]

use crate::marketplace::{MarketplaceIndex, MarketplaceMatch};
use crate::registry::ComponentMeta;

/// One ranked local match — a component **kind** the description resolved to, carrying its real
/// capabilities so the instantiated result is a working object, not dead geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    /// The matched `ComponentMeta` name (e.g. `"HealthBar"`).
    pub kind: String,
    /// Fit score in `[0,1]` (higher = better).
    pub score: f32,
    /// Capabilities the kind provides (attached on instantiate).
    pub provides: Vec<String>,
    /// Capabilities the kind requires (what it can attach to — drives the M3.1 one-click attach).
    pub requires: Vec<String>,
}

/// The tier a resolve would escalate to when the local library has no confident match. Marketplace +
/// Generate are **seams** (documented, not wired) — the happy path never needs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextTier {
    /// 0 local results → would query the (Phase-2) marketplace index.
    Marketplace,
}

/// A resolution: ranked local matches, and — if empty — the seam tier that would be tried next.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// Ranked local matches above the confidence threshold (may be empty).
    pub matches: Vec<Match>,
    /// `Some(Marketplace)` when there is no confident local match — the documented stub seam.
    pub next_tier: Option<NextTier>,
}

/// Minimum normalized score for a local match to count — below this we honestly report no match and
/// fall through to the marketplace seam, rather than return a wrong one.
const MIN_SCORE: f32 = 0.5;

/// A resolution across the full **local → marketplace → generate** order (ADR-012 + ADR-015). The
/// local tier is offline + deterministic and **short-circuits**: the marketplace index is queried only
/// when local has no confident match (so the offline happy path never touches the network), and the
/// generate tier stays a documented stub (no text-to-3D here).
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// A confident **local** match (ranked) — offline, deterministic; the marketplace was not queried.
    Local(Vec<Match>),
    /// No local match, but the **marketplace** index returned ranked pre-componentized entries.
    Marketplace(Vec<MarketplaceMatch>),
    /// No match anywhere — the **generate** seam (Phase-2 text-to-3D; unbuilt stub).
    Generate,
}

/// Resolve `query` across the real tiers: `resolve_local` first (offline), then — only if it found
/// nothing — the marketplace `index`, then the generate seam. The marketplace query is explicitly the
/// **second** tier and never runs on the offline happy path.
#[must_use]
pub fn resolve<I: MarketplaceIndex>(metas: &[ComponentMeta], index: &I, query: &str) -> Resolved {
    let local = resolve_local(metas, query);
    if !local.matches.is_empty() {
        return Resolved::Local(local.matches);
    }
    let market = index.query(query);
    if market.is_empty() {
        Resolved::Generate
    } else {
        Resolved::Marketplace(market)
    }
}

/// Resolve `query` against the local component library `metas` (e.g. `stdlib::standard_components()`).
/// Ranked best-first; empty + `next_tier = Marketplace` when nothing clears [`MIN_SCORE`].
#[must_use]
pub fn resolve_local(metas: &[ComponentMeta], query: &str) -> Resolution {
    let q = normalize(query);
    if q.is_empty() {
        return Resolution {
            matches: Vec::new(),
            next_tier: Some(NextTier::Marketplace),
        };
    }

    let mut matches: Vec<Match> = metas
        .iter()
        .filter_map(|m| {
            let score = score_meta(m, &q);
            (score >= MIN_SCORE).then(|| Match {
                kind: m.name.clone(),
                score,
                provides: m.provides.clone(),
                requires: m.requires.clone(),
            })
        })
        .collect();

    // Rank by score desc, then name asc for a stable, deterministic order.
    matches.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.kind.cmp(&b.kind))
    });

    let next_tier = matches.is_empty().then_some(NextTier::Marketplace);
    Resolution { matches, next_tier }
}

/// Score one kind against the normalized query tokens — weighted token overlap over its
/// camelCase-split name + aliases (weight 1.0), then tags + capabilities (weight 0.6), then
/// substring/prefix (0.3). Normalized by query length to `[0,1]`.
fn score_meta(m: &ComponentMeta, q: &[String]) -> f32 {
    // Strong tokens: the kind name (camelCase-split) + alias words.
    let mut strong: Vec<String> = split_name(&m.name);
    for a in &m.aliases {
        strong.extend(normalize(a));
    }
    // Weak tokens: tags + provided/required capabilities (capabilities lowercased + split).
    let mut weak: Vec<String> = m.tags.iter().flat_map(|t| normalize(t)).collect();
    for c in m.provides.iter().chain(m.requires.iter()) {
        weak.extend(split_name(c));
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

/// Lowercase, split on non-alphanumerics, drop stopwords, and apply a tiny synonym map so common
/// phrasings reach the curated vocabulary ("hp" → health, "sound" → audio, …). Shared with the
/// marketplace tier ([`crate::marketplace`]) so both tiers tokenize identically.
pub(crate) fn normalize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_ascii_lowercase)
        .filter(|t| !is_stopword(t))
        .map(|t| synonym(&t).to_string())
        .collect()
}

/// Split a `CamelCase`/`PascalCase` identifier into lowercase word tokens (`HealthBar` → [health, bar]).
/// Strips any `ns:` capability namespace first, so `acme:Shield` tokenizes as `shield` (the local name).
pub(crate) fn split_name(name: &str) -> Vec<String> {
    let name = crate::caps::local_name(name);
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in name.chars() {
        if ch.is_uppercase() && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if ch.is_alphanumeric() {
            cur.push(ch.to_ascii_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.into_iter().map(|t| synonym(&t).to_string()).collect()
}

fn is_stopword(t: &str) -> bool {
    matches!(
        t,
        "a" | "an" | "the" | "of" | "for" | "to" | "with" | "my" | "this"
    )
}

/// Tiny synonym normalization toward the curated stdlib vocabulary. Phase-2 (marketplace) replaces
/// this hand map with a learned/embedding index — see ADR-012.
fn synonym(t: &str) -> &str {
    match t {
        "hp" | "hitpoints" | "hitpoint" => "health",
        "sound" | "sfx" | "noise" => "audio",
        "rigidbody" | "rigid" => "physics",
        "lamp" | "lighting" => "light",
        "model" | "mesh3d" => "mesh",
        "behaviour" | "behavior" | "script" => "behavior",
        other => other,
    }
}

#[cfg(test)]
mod tier_tests {
    use super::*;
    use crate::marketplace::{LocalCatalog, MarketplaceEntry};
    use std::cell::Cell;

    /// A marketplace index that records whether it was queried — the "pull-the-network" guard.
    struct SpyIndex {
        inner: LocalCatalog,
        queried: Cell<usize>,
    }
    impl MarketplaceIndex for SpyIndex {
        fn query(&self, description: &str) -> Vec<MarketplaceMatch> {
            self.queried.set(self.queried.get() + 1);
            self.inner.query(description)
        }
        fn get(&self, id: &str) -> Option<MarketplaceEntry> {
            self.inner.get(id)
        }
    }
    fn spy() -> SpyIndex {
        SpyIndex {
            inner: LocalCatalog::builtin(),
            queried: Cell::new(0),
        }
    }

    #[test]
    fn local_hit_never_queries_the_marketplace() {
        let metas = crate::stdlib::standard_components();
        let idx = spy();
        let r = resolve(&metas, &idx, "health bar");
        assert!(matches!(r, Resolved::Local(_)));
        assert_eq!(
            idx.queried.get(),
            0,
            "the offline happy path must NOT touch the marketplace"
        );
    }

    #[test]
    fn no_local_match_falls_to_the_marketplace() {
        let metas = crate::stdlib::standard_components();
        let idx = spy();
        let r = resolve(&metas, &idx, "rusty medieval sword");
        assert_eq!(
            idx.queried.get(),
            1,
            "no local → the marketplace IS queried"
        );
        match r {
            Resolved::Marketplace(m) => assert_eq!(m[0].entry.id, "forge:rusty-sword"),
            other => panic!("expected the marketplace tier, got {other:?}"),
        }
    }

    #[test]
    fn no_match_anywhere_is_the_generate_seam() {
        let metas = crate::stdlib::standard_components();
        let idx = spy();
        let r = resolve(&metas, &idx, "zzqxw plumbus nonsense");
        assert!(matches!(r, Resolved::Generate));
        assert_eq!(idx.queried.get(), 1);
    }
}
