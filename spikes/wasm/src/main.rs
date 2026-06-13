//! Native entry point. On wasm32 the entry is `lib::wasm_main` (#[wasm_bindgen(start)]),
//! so this bin is a no-op there.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    metrocalk_wasm_spike::run();
}

#[cfg(target_arch = "wasm32")]
fn main() {}
