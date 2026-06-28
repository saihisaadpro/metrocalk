//! The project-owned `PluginHost` interface (invariant 5) — the ONLY surface the rest of the engine uses
//! to run a WASM plugin. **No `extism::` type crosses it** (the Extism backend lives in
//! [`crate::extism_host`], grep-gated, exactly as rapier lives in `/physics` and loro in `/core`). A
//! plugin is sandboxed — a memory + execution-time budget + a host-function allow-list — and any
//! misbehaviour (a trap, a panic, a timeout, an out-of-budget, a disallowed host call) is **contained +
//! explained** ([`PluginError`], ADR-016 spirit): a plugin can never crash the engine.

use std::time::Duration;
use thiserror::Error;

/// The sandbox limits applied to a plugin (M12.3 / ADR-047). A plugin gets ONLY what's granted here: a
/// bounded memory, a bounded execution time, and ONLY the host functions named in `allowed_host_fns`
/// (capability-namespacing, the ADR-015 spirit) — **no ambient scene / file / network access**.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sandbox {
    /// Max linear-memory pages (1 page = 64 KiB) — an over-allocating plugin **traps** (contained).
    pub max_memory_pages: u32,
    /// Wall-clock execution budget per call — a runaway plugin is **interrupted** (contained), not hung.
    pub timeout: Duration,
    /// A deterministic instruction budget (wasmtime "fuel"); `None` = unbounded by fuel (the wall-clock
    /// `timeout` still bounds it). Fuel is the **determinism-safe** bound for the Play/replay path (the
    /// same plugin + input burns the same fuel on every machine, unlike wall-clock `timeout`).
    pub fuel_limit: Option<u64>,
    /// The host-function allow-list — the ONLY host fns the plugin may import. Empty = a **pure, fully
    /// sandboxed** plugin (the example plugin's case): no ambient host access at all.
    pub allowed_host_fns: Vec<String>,
}

impl Sandbox {
    /// A restrictive default for an untrusted plugin: a small memory + time budget, a deterministic fuel
    /// cap, and **no** host functions (no ambient access).
    #[must_use]
    pub fn restrictive() -> Self {
        Self {
            max_memory_pages: 256, // 16 MiB
            timeout: Duration::from_millis(250),
            fuel_limit: Some(1_000_000_000), // ~1e9 instructions — generous for compute, bounds a runaway
            allowed_host_fns: Vec::new(),
        }
    }
}

/// Why a plugin op failed — every variant is **contained + explained** (ADR-016 spirit): a misbehaving
/// plugin surfaces a plain-language error, never an engine crash. No `extism::` type appears here.
#[derive(Error, Debug)]
pub enum PluginError {
    /// The plugin module failed to load / instantiate (bad wasm, or a missing / disallowed import).
    #[error("the plugin could not load: {0}")]
    Load(String),
    /// The plugin exceeded its time budget (a runaway loop) — interrupted, not hung.
    #[error("the plugin ran longer than its {0:?} budget and was stopped")]
    Timeout(Duration),
    /// The plugin exceeded its memory or fuel budget.
    #[error("the plugin exceeded its sandbox budget: {0}")]
    BudgetExceeded(String),
    /// The plugin tried to call a host function it wasn't granted (no ambient access).
    #[error("the plugin tried to use a capability it isn't allowed: {0}")]
    DisallowedHostFn(String),
    /// The exported function trapped / panicked at runtime (contained, not an engine crash).
    #[error("the plugin function '{func}' failed: {reason}")]
    Call { func: String, reason: String },
    /// The plugin produced output that wasn't the expected shape.
    #[error("the plugin produced invalid output: {0}")]
    BadOutput(String),
}

/// A loaded, sandboxed plugin instance.
pub trait PluginInstance {
    /// Run an exported `func` with `input` bytes, returning its output bytes. Any trap / timeout / OOM /
    /// disallowed-host-call is returned as a [`PluginError`] — it **never panics or crashes the host**.
    ///
    /// # Errors
    /// A [`PluginError`] describing the (contained) failure.
    fn call(&mut self, func: &str, input: &[u8]) -> Result<Vec<u8>, PluginError>;
}

/// A WASM plugin host (invariant 5). The single interface the engine uses to load a sandboxed plugin; no
/// host-implementation type (`extism::`) crosses it.
pub trait PluginHost {
    /// The loaded-instance type this host produces.
    type Instance: PluginInstance;

    /// Load + instantiate `wasm` under the `sandbox` budget + allow-list.
    ///
    /// # Errors
    /// [`PluginError::Load`] if the module is invalid or requests a disallowed import.
    fn load(&self, wasm: &[u8], sandbox: &Sandbox) -> Result<Self::Instance, PluginError>;
}
