//! M12.4 (ADR-048) — the editor's in-app **COMPOSE seam**: a natural-language sentence → a [`Composition`]
//! **proposal** the user reviews before it's applied through the ONE commit pipeline ([`apply_composition`]).
//! The AI is a **guest**: this only *proposes*; the deterministic engine validates + commits (one undoable
//! transaction) or refuses — a model can't reach past `apply_composition`.
//!
//! Mirrors the M6 generation seam (invariant 5): a project-owned trait with a deterministic **offline** demo
//! (the demo + e2e + tests, no network/model) and a **real LLM** provider as a **documented seam** — gated
//! behind a provider + API key, with the SA-22 [`composition_grammar`] as its **structured-output
//! constraint** (the model can only emit in-grammar ops). No provider SDK type crosses this boundary. The
//! **shipped** live path is the `metrocalk-mcp` server (an external MCP client like Claude); this in-editor
//! composer is the convenience seam beside it.
//!
//! [`apply_composition`]: metrocalk_core::apply_composition
//! [`composition_grammar`]: metrocalk_core::composition_grammar

use metrocalk_core::compose::{ComposeOp, Composition};
use metrocalk_core::rules::{Action, CompareOp, Condition, RuleData};
use metrocalk_core::FieldValue;

/// Why a compose proposal produced nothing — flattened (no foreign provider-error type leaks across the seam).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeAiError {
    /// Offline / not configured — the honest degradation (NEVER a fabricated composition).
    Unavailable(String),
    /// The provider was reached but the sentence couldn't be turned into an in-grammar composition.
    Provider(String),
}

impl std::fmt::Display for ComposeAiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(s) => write!(f, "compose unavailable: {s}"),
            Self::Provider(s) => write!(f, "compose provider error: {s}"),
        }
    }
}

impl std::error::Error for ComposeAiError {}

/// A project-owned natural-language → [`Composition`] provider (invariant 5). It only **proposes**; the
/// engine then validates + applies (or refuses) via `apply_composition` — the provider never touches the
/// engine. `target` is the entity the user selected for the rule to act on (Rules reference concrete
/// entities, ADR-045). A real impl wraps an LLM behind this trait, constrained by the SA-22 `grammar`; no SDK
/// type appears in its public surface.
pub trait Composer {
    /// A short identifier for logs / UX.
    fn name(&self) -> &'static str;
    /// Whether compose is available right now (configured + online). Offline/unconfigured ⇒ false, so the
    /// caller degrades to an honest "compose unavailable" seam (never a fabricated composition).
    fn available(&self) -> bool;
    /// Propose a composition for `sentence` acting on `target` (a loro entity key), guided by `grammar` (the
    /// SA-22 schema a real LLM constrains its structured output by). The result is still **validated** by the
    /// engine before anything is applied.
    ///
    /// # Errors
    /// [`ComposeAiError`] when unavailable or the sentence can't be composed in-grammar.
    fn propose(
        &self,
        sentence: &str,
        target: Option<&str>,
        grammar: &serde_json::Value,
    ) -> Result<Composition, ComposeAiError>;
}

/// A deterministic, **offline** demo composer (the demo + e2e + tests): turns a small set of recognizable
/// demo intents into the matching [`Composition`] with **no network and no model**. It does NOT pretend to be
/// a general LLM — an unrecognized sentence is an honest `Provider` "I only know the demo intents; configure
/// a real provider". So the offline path demonstrates *sentence → structured patches → the validated
/// pipeline* without faking open-ended understanding (the same honesty as the offline [`FakeGenerator`]).
///
/// [`FakeGenerator`]: crate::generate::FakeGenerator
#[derive(Clone, Default)]
pub struct DemoComposer {
    available: bool,
}

impl DemoComposer {
    /// A demo composer; `available=false` simulates offline (the honest "unavailable" seam).
    #[must_use]
    pub fn new(available: bool) -> Self {
        Self { available }
    }
}

impl Composer for DemoComposer {
    fn name(&self) -> &'static str {
        "demo"
    }
    fn available(&self) -> bool {
        self.available
    }
    fn propose(
        &self,
        sentence: &str,
        target: Option<&str>,
        _grammar: &serde_json::Value,
    ) -> Result<Composition, ComposeAiError> {
        if !self.available {
            return Err(ComposeAiError::Unavailable(
                "AI compose is offline — the metrocalk-mcp server (an MCP client like Claude) can compose \
                 against the grammar, or configure a provider"
                    .to_string(),
            ));
        }
        let s = sentence.to_lowercase();
        let target = target.ok_or_else(|| {
            ComposeAiError::Provider("select the entity the rule should act on first".to_string())
        })?;
        // Flagship demo intent: "when enemies die / kills reach N, set it on fire / ignite it / make it
        // flammable". A real LLM generalizes; the demo recognizes this one composition deterministically.
        let on_kill = s.contains("die")
            || s.contains("dies")
            || s.contains("kill")
            || s.contains("defeat")
            || s.contains("slain");
        let ignite = s.contains("fire")
            || s.contains("ignite")
            || s.contains("burn")
            || s.contains("flam")
            || s.contains("lit");
        if on_kill && ignite {
            let threshold = first_integer(&s).unwrap_or(4);
            return Ok(Composition {
                ops: vec![
                    // Seed the counter the rule reads, so its condition references a real, typed field.
                    ComposeOp::SetField {
                        entity: target.to_string(),
                        component: "KillCounter".to_string(),
                        field: "count".to_string(),
                        value: FieldValue::Integer(0),
                    },
                    ComposeOp::AuthorRule {
                        id: "r_ai_ignite".to_string(),
                        rule: ignite_rule(target, threshold),
                    },
                ],
            });
        }
        Err(ComposeAiError::Provider(format!(
            "the offline demo composer doesn't recognize \"{sentence}\" — it knows the ignite-on-kills demo; \
             configure a real LLM provider (or use the metrocalk-mcp server) for open-ended compose"
        )))
    }
}

/// The demo's flagship rule: **when** an enemy dies, **if** this entity's `KillCounter.count` ≥ `threshold`,
/// **then** set its `Flammable.lit` true. Targets a concrete entity (Rules reference real entities, ADR-045).
fn ignite_rule(target: &str, threshold: i64) -> RuleData {
    RuleData {
        name: "ignite on kills".to_string(),
        enabled: true,
        event: "EnemyDied".to_string(),
        conditions: vec![Condition {
            entity: target.to_string(),
            component: "KillCounter".to_string(),
            field: "count".to_string(),
            op: CompareOp::Ge,
            value: FieldValue::Integer(threshold),
        }],
        actions: vec![Action {
            action: "SetField".to_string(),
            entity: target.to_string(),
            component: "Flammable".to_string(),
            field: "lit".to_string(),
            value: FieldValue::Bool(true),
        }],
    }
}

/// The first run of ASCII digits in `s` as an `i64` (the demo's "reach N" threshold). `None` if there's none.
fn first_integer(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    let start = bytes.iter().position(u8::is_ascii_digit)?;
    let end = bytes[start..]
        .iter()
        .position(|b| !b.is_ascii_digit())
        .map_or(bytes.len(), |p| start + p);
    s[start..end].parse().ok()
}

/// The **real** LLM composer — a documented seam (not built). A real impl sends `sentence` + the registry
/// `grammar` to an LLM as a **structured-output constraint** (the model can emit only in-grammar ops), then
/// returns the [`Composition`] for the engine to validate + apply. Gated behind a provider + API key;
/// unconfigured ⇒ unavailable, so it never appears on a happy path until wired. The grammar is the SA-22
/// constraint; the engine is still the every-"no" gate (a model can't bypass `apply_composition`). The
/// **shipped** live path is the `metrocalk-mcp` server — this in-editor seam is the convenience beside it.
#[derive(Default)]
pub struct RemoteComposer {
    /// `true` once a provider endpoint + key are configured; `false` = the seam (unavailable).
    configured: bool,
}

impl Composer for RemoteComposer {
    fn name(&self) -> &'static str {
        "remote-llm"
    }
    fn available(&self) -> bool {
        self.configured
    }
    fn propose(
        &self,
        _sentence: &str,
        _target: Option<&str>,
        _grammar: &serde_json::Value,
    ) -> Result<Composition, ComposeAiError> {
        Err(ComposeAiError::Unavailable(
            "a real in-editor LLM compose provider is a documented seam — configure a provider + API key; \
             the shipped live path is the metrocalk-mcp server (an external MCP client like Claude)"
                .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grammar() -> serde_json::Value {
        metrocalk_core::composition_grammar(&metrocalk_core::stdlib::standard_components())
    }

    #[test]
    fn the_demo_composer_turns_the_flagship_sentence_into_an_ignite_composition() {
        let c = DemoComposer::new(true);
        let comp = c
            .propose(
                "when an enemy dies and kills reach 3, set it on fire",
                Some("1_5"),
                &grammar(),
            )
            .expect("the demo intent composes");
        // Seeds the counter + authors exactly the ignite rule, targeting the selected entity at threshold 3.
        assert_eq!(comp.ops.len(), 2);
        match &comp.ops[1] {
            ComposeOp::AuthorRule { rule, .. } => {
                assert_eq!(rule.event, "EnemyDied");
                assert_eq!(rule.conditions[0].entity, "1_5");
                assert_eq!(rule.conditions[0].value, FieldValue::Integer(3));
            }
            other => panic!("expected an AuthorRule, got {other:?}"),
        }
    }

    #[test]
    fn offline_is_an_honest_unavailable_not_a_fabrication() {
        let err = DemoComposer::new(false)
            .propose("anything", Some("1_0"), &grammar())
            .unwrap_err();
        assert!(matches!(err, ComposeAiError::Unavailable(_)), "{err}");
    }

    #[test]
    fn an_unrecognized_sentence_is_an_honest_provider_miss_not_a_guess() {
        let err = DemoComposer::new(true)
            .propose("paint the whole level purple", Some("1_0"), &grammar())
            .unwrap_err();
        assert!(matches!(err, ComposeAiError::Provider(_)), "{err}");
    }

    #[test]
    fn the_real_llm_provider_is_an_unconfigured_seam() {
        let c = RemoteComposer::default();
        assert!(!c.available());
        assert!(matches!(
            c.propose("x", Some("1_0"), &grammar()).unwrap_err(),
            ComposeAiError::Unavailable(_)
        ));
    }
}
