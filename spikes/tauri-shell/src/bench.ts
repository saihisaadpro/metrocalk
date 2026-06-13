// Sub-gate 1a — IPC delta-wire measurement (JS side).
//
// Three transports carry the REAL coalesced-delta encoding (Loro update bytes produced by
// metrocalk-core's commit pipeline, fetched from Rust via `gen_delta`):
//   - invoke raw-binary (JS→Rust→JS)  — measured on the JS clock (performance.now RTT)
//   - local WebSocket   (JS→Rust→JS)  — measured on the JS clock (CEF-style alternative)
//   - Tauri Channel     (Rust→JS push)— measured on the RUST clock; JS only acks (results from Rust)
//
// All three are RTT (one clock, no cross-clock skew). One-way ≈ RTT/2. The frame budget is
// 16.6 ms (60 Hz); the gate wants p99 end-to-end ≤ ~4 ms and p99 IPC budget ≤ ~5 ms with zero
// dropped frames over 60 s.
import { invoke, Channel } from "@tauri-apps/api/core";

const FRAME_MS = 1000 / 60; // 16.667
const DEFAULT_SECONDS = 60;

export interface PathStats {
  path: string;
  frames: number;
  dropped: number; // RTT > frame budget, or tick overrun
  rtt: Percentiles;
  jitter: Percentiles; // |actual interval − 16.6ms|
  payloadBytes: Percentiles;
}

export interface Percentiles {
  n: number;
  min: number;
  p50: number;
  p95: number;
  p99: number;
  max: number;
  mean: number;
}

function pct(xs: number[]): Percentiles {
  if (xs.length === 0) return { n: 0, min: 0, p50: 0, p95: 0, p99: 0, max: 0, mean: 0 };
  const s = [...xs].sort((a, b) => a - b);
  const at = (q: number) => s[Math.min(s.length - 1, Math.floor(q * s.length))];
  const mean = s.reduce((a, b) => a + b, 0) / s.length;
  return { n: s.length, min: s[0], p50: at(0.5), p95: at(0.95), p99: at(0.99), max: s[s.length - 1], mean };
}

// Run a fixed-rate 60 Hz loop for `seconds`, calling `tick` each frame. `tick` returns the RTT (ms)
// for that frame (or null if it failed). Records interval jitter and dropped frames.
async function drive(
  seconds: number,
  tick: (seq: number) => Promise<number | null>,
): Promise<{ rtt: number[]; jitter: number[]; dropped: number; frames: number }> {
  const rtt: number[] = [];
  const jitter: number[] = [];
  let dropped = 0;
  let frames = 0;
  const start = performance.now();
  let next = start;
  let last = start;
  let seq = 0;
  while (performance.now() - start < seconds * 1000) {
    const now = performance.now();
    // fixed-rate schedule: wait until the next 16.6ms slot
    if (now < next) {
      await sleep(next - now);
    }
    const fired = performance.now();
    jitter.push(Math.abs(fired - last - FRAME_MS));
    last = fired;
    next += FRAME_MS;
    frames++;
    const r = await tick(seq++);
    if (r === null || r > FRAME_MS) dropped++;
    if (r !== null) rtt.push(r);
    // if we've fallen behind by more than a frame, resync (counts as drops already)
    if (performance.now() > next + FRAME_MS) next = performance.now();
  }
  return { rtt, jitter, dropped, frames };
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, Math.max(0, ms)));

// --- Path 1: invoke raw-binary (JS→Rust→JS). Sends the real delta as raw bytes; Rust applies it
// to a mirror doc and returns. Pure JS-clock RTT. ---
async function benchInvoke(seconds: number, deltas: Uint8Array[]): Promise<PathStats> {
  const sizes: number[] = [];
  const res = await drive(seconds, async (seq) => {
    const payload = deltas[seq % deltas.length];
    sizes.push(payload.byteLength);
    const t0 = performance.now();
    try {
      // RAW binary: pass the Uint8Array directly so Tauri ships InvokeBody::Raw (no JSON encode).
      await invoke("commit_delta", payload);
    } catch {
      return null;
    }
    return performance.now() - t0;
  });
  return {
    path: "invoke-raw-binary",
    frames: res.frames,
    dropped: res.dropped,
    rtt: pct(res.rtt),
    jitter: pct(res.jitter),
    payloadBytes: pct(sizes),
  };
}

// --- Path 2: local WebSocket (JS→Rust→JS echo). The CEF-style transport. ---
async function benchWebSocket(seconds: number, deltas: Uint8Array[], port: number): Promise<PathStats> {
  const sizes: number[] = [];
  const ws = new WebSocket(`ws://127.0.0.1:${port}`);
  ws.binaryType = "arraybuffer";
  await new Promise<void>((resolve, reject) => {
    ws.onopen = () => resolve();
    ws.onerror = () => reject(new Error("ws connect failed"));
  });
  const pending = new Map<number, (rtt: number) => void>();
  ws.onmessage = (ev) => {
    // echo carries the 4-byte seq prefix we sent
    const view = new DataView(ev.data as ArrayBuffer);
    const seq = view.getUint32(0, true);
    const done = pending.get(seq);
    if (done) {
      pending.delete(seq);
      done(performance.now());
    }
  };
  const res = await drive(seconds, async (seq) => {
    const delta = deltas[seq % deltas.length];
    sizes.push(delta.byteLength);
    const buf = new Uint8Array(4 + delta.byteLength);
    new DataView(buf.buffer).setUint32(0, seq, true);
    buf.set(delta, 4);
    const t0 = performance.now();
    const rtt = await new Promise<number | null>((resolve) => {
      const to = setTimeout(() => {
        pending.delete(seq);
        resolve(null);
      }, 100);
      pending.set(seq, (t1) => {
        clearTimeout(to);
        resolve(t1 - t0);
      });
      ws.send(buf);
    });
    return rtt;
  });
  ws.close();
  return {
    path: "local-websocket",
    frames: res.frames,
    dropped: res.dropped,
    rtt: pct(res.rtt),
    jitter: pct(res.jitter),
    payloadBytes: pct(sizes),
  };
}

// --- Path 3: Tauri Channel (Rust→JS push). Rust drives the 60 Hz push and measures RTT on its own
// clock via our ack; JS just acks each delta as fast as it can. Returns Rust-side stats. ---
async function benchChannel(seconds: number): Promise<PathStats> {
  // Rust pushes RAW bytes (8-byte LE seq prefix + real delta); JS reads the seq and acks so Rust
  // can close the RTT on its own clock. Results (Rust-clock RTT) come back from the command.
  const onDelta = new Channel<ArrayBuffer>();
  onDelta.onmessage = (buf) => {
    const seq = Number(new DataView(buf).getBigUint64(0, true));
    void invoke("channel_ack", { seq });
  };
  return await invoke<PathStats>("run_channel_bench", { seconds, onDelta });
}

// --- Bandwidth sweep: 1 KB → 1 MB single payloads through invoke, to find the Windows MB/s ceiling. ---
async function sweep(): Promise<Array<{ bytes: number; rttMs: Percentiles; mbPerSec: number }>> {
  const sizes = [1 << 10, 4 << 10, 16 << 10, 64 << 10, 256 << 10, 1 << 20];
  const out: Array<{ bytes: number; rttMs: Percentiles; mbPerSec: number }> = [];
  for (const size of sizes) {
    const payload = new Uint8Array(size);
    const rtts: number[] = [];
    const reps = size >= 1 << 20 ? 30 : 60;
    for (let i = 0; i < reps; i++) {
      const t0 = performance.now();
      await invoke<ArrayBuffer>("echo_bytes", payload); // raw round-trip, no JSON
      rtts.push(performance.now() - t0);
    }
    const p = pct(rtts);
    // one-way ≈ rtt/2; MB/s over the median one-way
    const mbPerSec = size / 1e6 / (p.p50 / 2 / 1000);
    out.push({ bytes: size, rttMs: p, mbPerSec });
  }
  return out;
}

export async function runBench1a(runLabel: string, seconds: number = DEFAULT_SECONDS): Promise<void> {
  const out = document.getElementById("out")!;
  const log = (m: string) => {
    out.textContent += "\n" + m;
    // eslint-disable-next-line no-console
    console.log("[1a]", m);
  };
  out.textContent = `sub-gate 1a — IPC delta wire (${runLabel}, ${seconds}s/path)`;

  // Pre-generate a ring of REAL deltas (real drag steps from the core commit pipeline).
  log("generating real coalesced deltas from the core pipeline…");
  const deltas: Uint8Array[] = [];
  for (let i = 0; i < 120; i++) {
    const buf = await invoke<ArrayBuffer>("gen_delta"); // raw Response → ArrayBuffer
    deltas.push(new Uint8Array(buf));
  }
  const wsPort = await invoke<number>("ws_port");

  log("path 1/3 — invoke raw-binary…");
  const invokeStats = await benchInvoke(seconds, deltas);
  log("path 2/3 — local websocket…");
  const wsStats = await benchWebSocket(seconds, deltas, wsPort);
  log("path 3/3 — tauri channel (rust-driven)…");
  const channelStats = await benchChannel(seconds);
  log("bandwidth sweep 1KB→1MB…");
  const sweepStats = await sweep();

  const report = {
    runLabel,
    seconds,
    paths: [invokeStats, wsStats, channelStats],
    sweep: sweepStats,
    realDeltaSampleBytes: invokeStats.payloadBytes,
  };
  log("writing results + exiting…");
  await invoke("report_results", { json: JSON.stringify(report, null, 2) });
}
