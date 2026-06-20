//! The payment-provider seam (ADR-004/005) — token top-ups behind a project-owned trait (invariant 5),
//! a deterministic SANDBOX impl (no real money), and the REAL provider as a documented go-live seam.

use crate::model::tokens_for_cents;

/// A successful top-up — the tokens to grant + the provider's reference (audit) + the charged cents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopUpReceipt {
    /// Tokens to credit to the buyer.
    pub tokens: u32,
    /// The provider's transaction reference.
    pub provider_ref: String,
    /// The amount charged, in US cents.
    pub cents: u32,
}

/// Why a top-up produced no tokens — flattened, no foreign SDK error type leaks (invariant 5).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PayError {
    /// The provider is not configured / not gone-live (the real provider before the go-live gate).
    #[error("payments not configured: {0}")]
    NotConfigured(String),
    /// The provider was reached but declined the charge.
    #[error("payment declined: {0}")]
    Declined(String),
}

/// A project-owned payment provider for token top-ups ($10 ≈ 100 tokens). No foreign SDK type crosses
/// it (invariant 5). `live()` = a real-money provider that has passed the go-live gate.
pub trait PaymentProvider {
    /// A short identifier for logs/UX.
    fn name(&self) -> &'static str;
    /// Whether this provider moves REAL money (a sandbox is always `false`).
    fn live(&self) -> bool;
    /// Charge `cents` and return the tokens to grant.
    ///
    /// # Errors
    /// [`PayError`] when unconfigured (the real provider before go-live) or declined.
    fn charge(&self, cents: u32, ref_id: &str) -> Result<TopUpReceipt, PayError>;
}

/// The deterministic sandbox — no network, no real money; a valid amount always succeeds. Makes the
/// top-up→grant loop CI-testable. `live()` is `false`: it never moves real money.
#[derive(Clone, Copy, Debug, Default)]
pub struct SandboxProvider;

impl PaymentProvider for SandboxProvider {
    fn name(&self) -> &'static str {
        "sandbox"
    }
    fn live(&self) -> bool {
        false
    }
    fn charge(&self, cents: u32, ref_id: &str) -> Result<TopUpReceipt, PayError> {
        if cents == 0 {
            return Err(PayError::Declined("zero amount".to_string()));
        }
        Ok(TopUpReceipt {
            tokens: tokens_for_cents(cents),
            provider_ref: format!("sandbox-{ref_id}"),
            cents,
        })
    }
}

/// The REAL provider (e.g. Stripe) — a documented go-live seam. Unconfigured ⇒ `live()` is `false` and
/// every charge errors, so real settlement is never reached until the go-live gate is wired (config +
/// API key + the ADR-005 legal review / Stripe Connect). No code path here moves money.
#[derive(Clone, Copy, Debug, Default)]
pub struct StripeProvider {
    /// `true` once a Stripe account + the go-live flag are configured; `false` = the seam.
    pub configured: bool,
}

impl PaymentProvider for StripeProvider {
    fn name(&self) -> &'static str {
        "stripe"
    }
    fn live(&self) -> bool {
        self.configured
    }
    fn charge(&self, _cents: u32, _ref_id: &str) -> Result<TopUpReceipt, PayError> {
        Err(PayError::NotConfigured(
            "real settlement is a go-live seam — configure Stripe + the go-live flag (ADR-004/005 \
             legal review, Stripe Connect)"
                .to_string(),
        ))
    }
}

/// Select the payment provider by the go-live gate. Default (gate OFF) = the sandbox, so no test or
/// default build can reach a real charge; the real provider appears only behind an explicit go-live —
/// and even then its `charge` is the unbuilt seam (it errors), so no money moves in this codebase.
#[must_use]
pub fn select_provider(go_live: bool) -> Box<dyn PaymentProvider> {
    if go_live {
        Box::new(StripeProvider { configured: true })
    } else {
        Box::new(SandboxProvider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_grants_tokens_for_a_valid_charge_without_real_money() {
        let p = SandboxProvider;
        assert!(!p.live(), "the sandbox never moves real money");
        let r = p
            .charge(1000, "op-1")
            .expect("sandbox always succeeds for a valid amount");
        assert_eq!(r.tokens, 100, "$10 ⇒ 100 tokens");
        assert_eq!(r.provider_ref, "sandbox-op-1");
    }

    #[test]
    fn the_real_provider_is_an_unbuilt_go_live_seam() {
        let p = StripeProvider::default();
        assert!(!p.live(), "unconfigured Stripe is not live");
        assert!(matches!(
            p.charge(1000, "op-1"),
            Err(PayError::NotConfigured(_))
        ));
    }

    #[test]
    fn default_provider_selection_is_the_sandbox_so_no_real_charge_is_reachable() {
        // The go-live gate OFF selects the sandbox by SELECTION (not just availability) — a misconfigured
        // env can never construct a live charge path in tests.
        let p = select_provider(false);
        assert_eq!(p.name(), "sandbox");
        assert!(!p.live());
        // Even when "gone live", the real provider's charge is still the unbuilt seam (no money moves).
        let live = select_provider(true);
        assert_eq!(live.name(), "stripe");
        assert!(live.charge(1000, "op").is_err());
    }
}
