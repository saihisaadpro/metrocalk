//! Sub-gate 1a — IPC delta-wire: Tauri commands + local WebSocket echo server + stats.
//!
//! The real deltas are pre-generated at startup (see `main`) into a byte ring and handed here, so
//! the (possibly `!Send`) Flecs world never enters Tauri's `Send + Sync` managed state — the bytes
//! are what the wire carries regardless.

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use tauri::ipc::{Channel, InvokeBody, InvokeResponseBody, Request, Response};
use tauri::{AppHandle, State};

pub struct BenchState {
    deltas: Vec<Vec<u8>>,
    delta_idx: AtomicUsize,
    mirror: Mutex<loro::LoroDoc>, // receive-side decode+integrate target
    ws_port: u16,
    run_label: String,
    channel_sends: Mutex<HashMap<u64, Instant>>,
    channel_rtts: Mutex<Vec<f64>>,
}

impl BenchState {
    pub fn new(deltas: Vec<Vec<u8>>, ws_port: u16, run_label: String) -> Self {
        Self {
            deltas,
            delta_idx: AtomicUsize::new(0),
            mirror: Mutex::new(loro::LoroDoc::new()),
            ws_port,
            run_label,
            channel_sends: Mutex::new(HashMap::new()),
            channel_rtts: Mutex::new(Vec::new()),
        }
    }

    pub fn run_label(&self) -> &str {
        &self.run_label
    }
}

#[derive(Serialize)]
struct Pctl {
    n: usize,
    min: f64,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
    mean: f64,
}

fn pctl(xs: &[f64]) -> Pctl {
    if xs.is_empty() {
        return Pctl { n: 0, min: 0.0, p50: 0.0, p95: 0.0, p99: 0.0, max: 0.0, mean: 0.0 };
    }
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = |q: f64| s[((q * s.len() as f64) as usize).min(s.len() - 1)];
    let mean = s.iter().sum::<f64>() / s.len() as f64;
    Pctl { n: s.len(), min: s[0], p50: at(0.5), p95: at(0.95), p99: at(0.99), max: s[s.len() - 1], mean }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathStats {
    path: String,
    frames: u64,
    dropped: u64,
    rtt: Pctl,
    jitter: Pctl,
    payload_bytes: Pctl,
}

fn raw_bytes(req: &Request<'_>) -> Vec<u8> {
    match req.body() {
        InvokeBody::Raw(b) => b.to_vec(),
        InvokeBody::Json(_) => Vec::new(),
    }
}

/// Serve the next real delta from the pre-generated ring (raw bytes → ArrayBuffer in JS).
#[tauri::command]
pub fn gen_delta(state: State<'_, BenchState>) -> Response {
    if state.deltas.is_empty() {
        return Response::new(Vec::<u8>::new());
    }
    let i = state.delta_idx.fetch_add(1, Ordering::Relaxed) % state.deltas.len();
    Response::new(state.deltas[i].clone())
}

/// invoke raw-binary sink (JS→Rust): decode+integrate the delta into the mirror CRDT.
#[tauri::command]
pub fn commit_delta(request: Request<'_>, state: State<'_, BenchState>) -> Result<(), String> {
    let bytes = raw_bytes(&request);
    let _ = state.mirror.lock().unwrap().import(&bytes);
    Ok(())
}

/// Raw echo (for the 1 KB→1 MB bandwidth sweep).
#[tauri::command]
pub fn echo_bytes(request: Request<'_>) -> Response {
    Response::new(raw_bytes(&request))
}

#[tauri::command]
pub fn ws_port(state: State<'_, BenchState>) -> u16 {
    state.ws_port
}

/// JS acks a channel-pushed delta; we stamp the RTT close on the Rust clock.
#[tauri::command]
pub fn channel_ack(seq: u64, state: State<'_, BenchState>) {
    let t1 = Instant::now();
    if let Some(t0) = state.channel_sends.lock().unwrap().remove(&seq) {
        state.channel_rtts.lock().unwrap().push((t1 - t0).as_secs_f64() * 1000.0);
    }
}

/// Tauri Channel path (Rust→JS push). Rust drives the 60 Hz loop and measures RTT on its own clock
/// via `channel_ack`. Payload = 8-byte LE seq prefix + the real delta, sent RAW (no JSON bloat).
#[tauri::command]
pub async fn run_channel_bench(
    seconds: f64,
    on_delta: Channel<InvokeResponseBody>,
    state: State<'_, BenchState>,
) -> Result<PathStats, String> {
    state.channel_sends.lock().unwrap().clear();
    state.channel_rtts.lock().unwrap().clear();
    let frame = std::time::Duration::from_secs_f64(1.0 / 60.0);
    let start = Instant::now();
    let mut next = start + frame;
    let mut last = start;
    let mut seq: u64 = 0;
    let mut jitter = Vec::new();
    let mut sizes = Vec::new();
    let ring = state.deltas.len().max(1);
    while start.elapsed().as_secs_f64() < seconds {
        let delta = &state.deltas[(seq as usize) % ring];
        sizes.push(delta.len() as f64);
        let mut payload = Vec::with_capacity(8 + delta.len());
        payload.extend_from_slice(&seq.to_le_bytes());
        payload.extend_from_slice(delta);
        let now = Instant::now();
        jitter.push(((now - last).as_secs_f64() * 1000.0 - 1000.0 / 60.0).abs());
        last = now;
        state.channel_sends.lock().unwrap().insert(seq, now);
        on_delta
            .send(InvokeResponseBody::Raw(payload))
            .map_err(|e| e.to_string())?;
        seq += 1;
        // Precise 60 Hz pacing. Windows' default timer quantum (~15 ms) makes `tokio::time::sleep`
        // overshoot a 16.6 ms target to ~31 ms (→ 35 Hz), so spin to the deadline instead. Yield to
        // the runtime ~1×/s as insurance; ack handling runs on the sync-command thread pool anyway.
        while Instant::now() < next {
            std::hint::spin_loop();
        }
        next += frame;
        if seq % 60 == 0 {
            tokio::task::yield_now().await;
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await; // drain stragglers
    let rtts = state.channel_rtts.lock().unwrap().clone();
    let frames = seq;
    let dropped = frames.saturating_sub(rtts.len() as u64);
    Ok(PathStats {
        path: "tauri-channel".into(),
        frames,
        dropped,
        rtt: pctl(&rtts),
        jitter: pctl(&jitter),
        payload_bytes: pctl(&sizes),
    })
}

/// Write the JS-assembled report to `_gate_out/1a-<run>.json` and exit.
#[tauri::command]
pub fn report_results(json: String, app: AppHandle, state: State<'_, BenchState>) -> Result<(), String> {
    let dir = std::path::Path::new("_gate_out");
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("1a-{}.json", state.run_label));
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    eprintln!("[bench] wrote {} — exiting", path.display());
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        app2.exit(0);
    });
    Ok(())
}

/// Local-WebSocket echo server (CEF-style transport). Binary frames echoed verbatim; the JS side
/// stamps RTT on its own clock.
pub async fn ws_echo_server(listener: std::net::TcpListener) {
    let listener = match tokio::net::TcpListener::from_std(listener) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[ws] adopt listener failed: {e}");
            return;
        }
    };
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(async move {
                    if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                        let (mut tx, mut rx) = ws.split();
                        while let Some(Ok(msg)) = rx.next().await {
                            if msg.is_binary() && tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                    }
                });
            }
            Err(_) => break,
        }
    }
}
