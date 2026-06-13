# Sub-gate 1a — IPC delta wire: results

**Environment (pinned):** Windows 11, WebView2 Evergreen **149.0.4022.69**, Tauri 2.11.2, wry 0.55.1.
Two independent 60 s runs per the orchestrator discipline (`1a-run-1.json`, `1a-run-2.json`).
Payload = the **real** coalesced-delta encoding (Loro update bytes from `core::Engine`, a 60 Hz
transform drag through the actual commit pipeline). All three transports carry **raw binary**
(no JSON re-encoding). RTT = full round trip on one clock; one-way ≈ RTT/2.

## Latency (round-trip ms, 3600 frames/path/run)

| transport | run | p50 | p95 | **p99** | max | dropped | sustained |
|---|---|---|---|---|---|---|---|
| **tauri-channel** (engine→UI push) | 1 | 2.36 | 2.84 | **3.55** | 8.87 | 0 | 60 Hz (3600 fr) |
| | 2 | 2.37 | 2.89 | **3.40** | 23.33 | 0 | 60 Hz |
| **local-websocket** (CEF-style) | 1 | 0.80 | 1.20 | **1.70** | 3.40 | 0 | 60 Hz (3601 fr) |
| | 2 | 0.80 | 1.10 | **1.30** | 29.00 | 1 | 60 Hz |
| **invoke raw-binary** (UI→engine) | 1 | 4.80 | 5.90 | **6.80** | 33.60 | 2 | 60 Hz |
| | 2 | 4.80 | 5.90 | **6.70** | 14.70 | 0 | 60 Hz |

Channel frame-interval jitter after the spin-wait pacing fix: p99 **0.03 ms** (essentially perfect
60 Hz). Real delta size: **103 bytes** p50 (105 max) — both runs.

## Bandwidth sweep (raw `invoke` echo, one-way MB/s)

| payload | p50 RTT | MB/s | | payload | p50 RTT | MB/s |
|---|---|---|---|---|---|---|
| 1 KB | 3.7 ms | 0.55 | | 64 KB | 7.2 ms | 18.2 |
| 4 KB | 3.5 ms | 2.34 | | 256 KB | 19.2 ms | 27.3 |
| 16 KB | 4.3 ms | 7.6 | | 1 MB | 66 ms | **~31.8** |

The 1–16 KB rows show a **flat ~4 ms floor** — that's Tauri `invoke` per-call overhead, not
bandwidth. Bandwidth only dominates past ~256 KB.

## Verdict: **1a PASS**

- The transports that carry 60 Hz render-deltas — **Channel** (p99 ≈ 3.5 ms RTT) and **WebSocket**
  (p99 ≈ 1.5 ms RTT) — sustain 60 Hz with **zero / near-zero dropped frames**, clearing the
  ≤ 4 ms end-to-end and ≤ 5 ms IPC-budget bars on the **conservative RTT reading** (one-way is half).
- **Research correction confirmed:** the real delta is **103 bytes**, ~10,000× under the measured
  ~31.8 MB/s ceiling. The wire is **overhead-bound, not bandwidth-bound** — ADR-003's "~200 ms /
  10 MB" worst-case never applies to a deltas-only wire.
- **Caveat (honest):** the request/response `invoke` path carries a **~4 ms fixed per-call
  overhead** (p99 ≈ 6.8 ms RTT). Fine for *episodic* UI→engine commits, but it is the slowest path
  and should not be the 60 Hz channel — use Channel/WebSocket for streaming deltas.
- **Min-spec caveat:** measured on the dev box (see hardware in the run JSON / ADR). The numbers
  carry hundreds-of-× headroom, so even a 3–5× slower min-spec machine stays within frame budget on
  the streaming paths — but this is argued, not measured on min-spec.
