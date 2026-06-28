//! The **Extism backend** for [`PluginHost`] — the ONLY module where `extism::` appears (invariant 5,
//! grep-gated, exactly like rapier-in-`/physics`). It maps our [`Sandbox`] to Extism's manifest/builder
//! (memory cap + wall-clock timeout + a deterministic fuel cap + `with_wasi(false)` + ONLY the
//! allow-listed host fns) and maps every Extism error into a **contained** [`PluginError`]. Native-only
//! (wasmtime); the browser plugin host is a named seam (Extism's JS SDK), not built here.

use std::time::Duration;

use extism::{Manifest, Plugin, PluginBuilder, Wasm};

use crate::host::{PluginError, PluginHost, PluginInstance, Sandbox};

/// The native (wasmtime) Extism plugin host.
#[derive(Default)]
pub struct ExtismHost;

impl ExtismHost {
    /// A new native Extism host.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl PluginHost for ExtismHost {
    type Instance = ExtismPlugin;

    fn load(&self, wasm: &[u8], sandbox: &Sandbox) -> Result<ExtismPlugin, PluginError> {
        // The manifest carries the SANDBOX: a memory cap + a wall-clock timeout. `allowed_hosts` /
        // `allowed_paths` are left EMPTY → no ambient http / filesystem. (Extism's built-in http/fs host
        // fns aren't compiled in either — the crate is `default-features = false`.)
        let manifest = Manifest::new([Wasm::data(wasm.to_vec())])
            .with_memory_max(sandbox.max_memory_pages)
            .with_timeout(sandbox.timeout);

        // The allow-list IS the capability boundary: the plugin gets ONLY the host fns named here, and
        // importing any other fails to link (contained by omission). `build_host_fns` is **fail-closed** —
        // granting a capability the host doesn't define is a misconfiguration, refused (not silently
        // granted nothing). With `with_wasi(false)` there is no ambient WASI either (no clock/env/fs).
        let granted = build_host_fns(&sandbox.allowed_host_fns)?;
        let mut builder = PluginBuilder::new(manifest)
            .with_wasi(false)
            .with_functions(granted);
        if let Some(fuel) = sandbox.fuel_limit {
            builder = builder.with_fuel_limit(fuel);
        }

        let plugin = builder
            .build()
            .map_err(|e| PluginError::Load(e.to_string()))?;
        Ok(ExtismPlugin {
            plugin,
            timeout: sandbox.timeout,
        })
    }
}

/// A loaded Extism plugin. `extism::Plugin` stays **private** — it never crosses the trait boundary.
pub struct ExtismPlugin {
    plugin: Plugin,
    timeout: Duration,
}

impl PluginInstance for ExtismPlugin {
    fn call(&mut self, func: &str, input: &[u8]) -> Result<Vec<u8>, PluginError> {
        // Extism catches wasm traps + host-fn panics + the timeout/fuel interrupt and returns `Err` — so
        // a misbehaving plugin is contained here, never a host crash. We categorize the error for an
        // explained message; the raw reason always carries.
        match self.plugin.call::<&[u8], Vec<u8>>(func, input) {
            Ok(out) => Ok(out),
            Err(e) => Err(classify(func, self.timeout, &e)),
        }
    }
}

/// Resolve the allow-listed capability NAMES to the host [`extism::Function`]s to register — the capability
/// boundary. **Fail-closed:** M12.3 defines **no** host-fn vocabulary (the example plugin is pure compute),
/// so any non-empty grant names a capability the host can't provide and is **refused** (`DisallowedHostFn`)
/// rather than silently granting nothing. A granted vocabulary (a `log`, a scene-query capability) is wired
/// here in M12.4+ as match arms returning real `Function`s; an un-granted import then can't link (contained).
fn build_host_fns(allowed: &[String]) -> Result<Vec<extism::Function>, PluginError> {
    // No host-fn capability vocabulary is defined in M12.3 (the example plugin is pure), so ANY grant names
    // an unknown capability — fail closed. M12.4+ replaces this with a match that returns real `Function`s
    // for the granted names (a `log`, a scene-query) and refuses only the unknown ones.
    if let Some(unknown) = allowed.first() {
        return Err(PluginError::DisallowedHostFn(format!(
            "no such host capability to grant: '{unknown}'"
        )));
    }
    Ok(Vec::new())
}

/// Map an Extism (anyhow) error into a contained, explained [`PluginError`] by inspecting its message
/// (Extism doesn't expose typed trap kinds). Best-effort categorization; the raw reason always carries.
fn classify(func: &str, timeout: Duration, e: &extism::Error) -> PluginError {
    let msg = e.to_string();
    let low = msg.to_lowercase();
    if low.contains("timeout")
        || low.contains("deadline")
        || low.contains("interrupt")
        || low.contains("epoch")
    {
        PluginError::Timeout(timeout)
    } else if low.contains("fuel")
        || low.contains("out of memory")
        || low.contains("memory")
        || low.contains("grow")
    {
        PluginError::BudgetExceeded(msg)
    } else if low.contains("unknown import") || low.contains("link") || low.contains("imported") {
        PluginError::DisallowedHostFn(msg)
    } else {
        PluginError::Call {
            func: func.to_string(),
            reason: msg,
        }
    }
}
