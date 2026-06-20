//! The **generation tier** — the metered last resort (`local → marketplace → generate`, M6/ADR-012).
//!
//! Generation is the **guest**: never on the offline happy path, always last, always metered. It needs
//! the network + a paid provider, so it lives behind a project-owned trait ([`MeshGenerator`],
//! invariant 5) with a **deterministic offline fake** (tests + the offline demo) and a **real impl as a
//! documented seam** (gated behind config/API key). A generated mesh comes back as **glb bytes** and is
//! normalized through the prompt-23 importer by the caller — generation is a *source* of assets, not a
//! second asset path; no foreign provider SDK type crosses this boundary.
//!
//! Token metering is the [`TokenMeter`] seam (ADR-004): the stub records a cost + always allows + logs;
//! the real ledger/grants/settlement land in prompt 26. **No money moves here.**

use std::time::Duration;

/// A generation request — a text prompt (+ room for structured hints later). No foreign types.
#[derive(Debug, Clone)]
pub struct GenRequest {
    pub prompt: String,
}

impl GenRequest {
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
        }
    }
}

/// Why a generation produced no asset — flattened, no foreign provider-error type leaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenError {
    /// Offline / not configured (the honest degradation — never a fake result).
    Unavailable(String),
    /// The provider was reached but failed.
    Provider(String),
}

impl std::fmt::Display for GenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(s) => write!(f, "generation unavailable: {s}"),
            Self::Provider(s) => write!(f, "generation provider error: {s}"),
        }
    }
}

impl std::error::Error for GenError {}

/// A project-owned text-to-3D provider (invariant 5). Returns glb **bytes**; the caller imports them
/// through the prompt-23 pipeline (validation + size limits apply there). A real impl wraps a provider
/// SDK behind this trait — no SDK types in its public surface.
pub trait MeshGenerator {
    /// A short identifier for logs/UX.
    fn name(&self) -> &'static str;
    /// Whether generation is available right now (configured + online). Offline/unconfigured ⇒ false,
    /// so the caller degrades to an honest "generation unavailable" seam.
    fn available(&self) -> bool;
    /// Produce glb bytes for `req` (blocking — the caller runs it OFF the hot path / on a worker).
    ///
    /// # Errors
    /// [`GenError`] when unavailable or the provider fails.
    fn generate(&self, req: &GenRequest) -> Result<Vec<u8>, GenError>;
}

/// A deterministic, **offline** fake generator (tests + the offline demo): returns caller-provided
/// checked-in mesh bytes after a simulated latency — so the placeholder→stream-in loop, the
/// transactional apply, and offline-degradation are CI-testable without a network/provider. Because the
/// bytes are a fixed checked-in mesh, the generated asset's content-address is **stable**, so a replayed
/// generation lands the same asset (ADR-013).
#[derive(Clone)]
pub struct FakeGenerator {
    mesh_bytes: Vec<u8>,
    latency: Duration,
    available: bool,
}

impl FakeGenerator {
    /// A fake that returns `mesh_bytes` after `latency`. `available=false` simulates offline.
    #[must_use]
    pub fn new(mesh_bytes: Vec<u8>, latency: Duration, available: bool) -> Self {
        Self {
            mesh_bytes,
            latency,
            available,
        }
    }
}

impl MeshGenerator for FakeGenerator {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn available(&self) -> bool {
        self.available
    }
    fn generate(&self, _req: &GenRequest) -> Result<Vec<u8>, GenError> {
        if !self.available {
            return Err(GenError::Unavailable(
                "generation unavailable offline".to_string(),
            ));
        }
        if !self.latency.is_zero() {
            std::thread::sleep(self.latency); // simulate provider round-trip (off the hot path)
        }
        Ok(self.mesh_bytes.clone())
    }
}

/// The **real** provider — a documented seam (not built). A real impl wraps a text-to-3D provider SDK
/// behind [`MeshGenerator`], gated behind an explicit config/API key. Unconfigured ⇒ unavailable, so it
/// never appears on a happy path until wired.
#[derive(Default)]
pub struct RemoteGenerator {
    /// `true` once a provider endpoint + key are configured; `false` = the seam (unavailable).
    configured: bool,
}

impl MeshGenerator for RemoteGenerator {
    fn name(&self) -> &'static str {
        "remote"
    }
    fn available(&self) -> bool {
        self.configured
    }
    fn generate(&self, _req: &GenRequest) -> Result<Vec<u8>, GenError> {
        Err(GenError::Unavailable(
            "real text-to-3D provider is a documented seam — configure a provider + API key (prompt 26+)"
                .to_string(),
        ))
    }
}

/// A metered action and its token cost class (ADR-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterAction {
    /// Fresh text-to-3D generation (≈ 10 tokens).
    Generate,
    /// LLM edit of an existing asset/entity (≈ 1–2 tokens).
    Edit,
}

/// The token-metering seam (ADR-004 — the real ledger lands in prompt 26). Records a cost + checks a
/// balance; the stub always allows + logs. **No money moves.**
pub trait TokenMeter {
    /// The token cost of an action.
    fn cost(&self, action: MeterAction) -> u32;
    /// Check + record a charge for `action` (labelled for the log). `Ok(cost)` if allowed, `Err(reason)`
    /// on insufficient balance.
    ///
    /// # Errors
    /// A human reason when the (future) balance is insufficient — the stub never errors.
    fn charge(&self, action: MeterAction, label: &str) -> Result<u32, String>;
}

/// The metering stub: canonical costs, always allows, logs. No ledger, no settlement, no money.
#[derive(Default)]
pub struct StubMeter;

impl TokenMeter for StubMeter {
    fn cost(&self, action: MeterAction) -> u32 {
        match action {
            MeterAction::Generate => 10,
            MeterAction::Edit => 2,
        }
    }
    fn charge(&self, action: MeterAction, label: &str) -> Result<u32, String> {
        let cost = self.cost(action);
        eprintln!("[meter] {label}: ≈{cost} tokens ({action:?}) — stub, no ledger / no charge");
        Ok(cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_generator_returns_its_bytes_when_available() {
        let g = FakeGenerator::new(vec![1, 2, 3, 4], Duration::ZERO, true);
        assert!(g.available());
        assert_eq!(
            g.generate(&GenRequest::new("a rock")).unwrap(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn offline_fake_degrades_honestly() {
        let g = FakeGenerator::new(vec![1, 2, 3], Duration::ZERO, false);
        assert!(!g.available());
        assert!(matches!(
            g.generate(&GenRequest::new("x")),
            Err(GenError::Unavailable(_))
        ));
    }

    #[test]
    fn real_provider_is_an_unavailable_seam() {
        let g = RemoteGenerator::default();
        assert!(!g.available());
        assert!(matches!(
            g.generate(&GenRequest::new("x")),
            Err(GenError::Unavailable(_))
        ));
    }

    #[test]
    fn meter_costs_and_logs_without_charging() {
        let m = StubMeter;
        assert_eq!(m.cost(MeterAction::Generate), 10);
        assert_eq!(m.cost(MeterAction::Edit), 2);
        assert_eq!(m.charge(MeterAction::Generate, "gen 'a rock'"), Ok(10));
    }
}
