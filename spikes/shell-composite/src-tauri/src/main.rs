#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! M2.1 Tauri/WebView2 exit-gate spike — entry point.
//!
//! `GATE_MODE` env selects the gate (default `selftest`):
//!   - `selftest`  : prove the JS↔Rust roundtrip + report the WebView2 runtime version.
//!   - `bench`     : sub-gate 1a (IPC delta wire). `RUN` env labels the output (run-1 / run-2).
//!   - `composite` : sub-gate 1b (transparent webview over wgpu) — layered in next.

mod bench;
mod composite;
mod deltas;

fn mode() -> String {
    std::env::var("GATE_MODE").unwrap_or_else(|_| "selftest".to_string())
}
fn run_label() -> String {
    std::env::var("RUN").unwrap_or_else(|_| "run-1".to_string())
}

#[tauri::command]
fn selftest(state: tauri::State<'_, bench::BenchState>) -> serde_json::Value {
    let wv = tauri::webview_version().unwrap_or_else(|_| "unknown".to_string());
    let m = mode();
    let seconds: f64 = std::env::var("BENCH_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(60.0);
    eprintln!("[selftest] invoked — webview2={wv} mode={m} run={} seconds={seconds}", state.run_label());
    serde_json::json!({ "ok": true, "webview2": wv, "mode": m, "run": state.run_label(), "seconds": seconds })
}

fn main() {
    let m = mode();
    eprintln!("[spike] starting — GATE_MODE={m} RUN={}", run_label());

    // Pre-generate REAL deltas from the core pipeline up-front (keeps the possibly-!Send Flecs world
    // out of Tauri's Send+Sync managed state; the byte ring is what the wire carries anyway).
    let deltas: Vec<Vec<u8>> = if m == "bench" {
        let mut g = deltas::DeltaGen::new();
        let ds: Vec<Vec<u8>> = (0..600).map(|_| g.step()).collect();
        eprintln!(
            "[spike] pre-generated {} real deltas (first sizes: {:?} bytes)",
            ds.len(),
            ds.iter().take(6).map(Vec::len).collect::<Vec<_>>()
        );
        ds
    } else {
        Vec::new()
    };

    // Bind the WS listener now so the port is known before the frontend asks for it.
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ws bind");
    std_listener.set_nonblocking(true).expect("ws nonblocking");
    let ws_port = std_listener.local_addr().unwrap().port();
    eprintln!("[spike] ws echo server on 127.0.0.1:{ws_port}");

    let state = bench::BenchState::new(deltas, ws_port, run_label());

    tauri::Builder::default()
        .manage(state)
        .setup(move |app| {
            tauri::async_runtime::spawn(bench::ws_echo_server(std_listener));
            // COMPOSITE_NOWGPU=1 runs the transparent webview WITHOUT the wgpu layer — the control
            // that isolates whether GDI can capture the webview at all (vs the wgpu surface breaking
            // / occluding it). Used to disambiguate the all-black automated capture.
            if m == "composite" && std::env::var("COMPOSITE_NOWGPU").as_deref() != Ok("1") {
                composite::start_from_app(&app.handle().clone());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            selftest,
            bench::gen_delta,
            bench::commit_delta,
            bench::echo_bytes,
            bench::ws_port,
            bench::run_channel_bench,
            bench::channel_ack,
            bench::report_results
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
