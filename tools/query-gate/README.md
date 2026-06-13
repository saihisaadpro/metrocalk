# query-gate — the compatibility-query performance gate (M1.5)

`query-gate` is the CI tripwire for **north-star test #1**: an edit must re-answer
*"what's compatible now?"* within **one 60 Hz frame (< 16 ms)**. It runs the M1.2
`World`-wrapper's **cached** compatibility query on M1.4's shared **5k preset**
(`scene::build_scene` / `preset_5k` / `compat_clauses` — the *same* fixture the tests
and `scene-bench` use, not a bespoke scene), measures p99 over 2000 samples, and
**exits non-zero if p99 > 16 ms**. `.github/workflows/perf-gate.yml` runs it on every
push + PR, so a regression past budget fails the build — the same discipline spike ③
gave the wasm build.

Run it locally (release is mandatory — debug timings are meaningless):

```
cargo run --release -p query-gate
```

## Calibration — why the threshold sits at 16 ms

**The gate is the absolute product budget, not `baseline × k`.** 16 ms is one 60 Hz
frame — the number that actually defines the interactivity promise. We deliberately do
*not* gate at "a little above the measured baseline" (see why below).

**Runner baseline (measured, never hand-typed).** Read back from the gate's own
`::notice::` annotation on real CI runs (`ubuntu-latest`, 2 vCPU):

| environment | median | p99 | max | budget | headroom @ p99 |
|---|---|---|---|---|---|
| **runner** (ubuntu-latest, 2 vCPU) | 10.6 µs | **20.6 µs** | 35.4 µs | 16 000 µs | **776×** |
| dev (Windows, desktop class) | 9.7 µs | 28.1 µs | 135 µs | 16 000 µs | ~570× |

The runner is *slower hardware than the i9 the M0 spikes used*, yet the cached query is
hundreds× under budget — the query is `O(matches)`, not `O(entities)`, and the compiled
query is cached (ADR-001 / 006).

**Why such enormous headroom is correct, not lazy.** At the µs scale, p99 on a shared
2-core CI VM is **noisy in relative terms** — a neighbour-VM hiccup can swing the tail
run-to-run — while being *utterly irrelevant* against a 16 ms budget. A `baseline × 3`
gate (~60 µs) would be **flaky**: it would fire on runner jitter, not on real
regressions. The 16 ms product-budget gate is **rock-solid** (hundreds× margin) yet
still catches the regressions that would actually break the promise — an accidental
**uncached** query path, or an archetype-**fragmentation** blowup — both of which are
*orders of magnitude*, not microseconds. The gate's job is to catch the cliff, not to
track micro-optimisation.

**Why an absolute gate and not `criterion`.** `criterion` detects *relative* regressions
against a stored baseline — excellent for tracking micro-opts, but it needs a committed
baseline and is sensitive to exactly the runner noise above. We want an *absolute* gate
tied to the frame budget: meaningful on any runner, no stored baseline, immune to µs
jitter. So the gate is a small release binary that measures p99 and `exit(1)` over
budget — the same posture as `tools/scene-bench`.

## Proving the gate goes red

Two env knobs (CI normally sets neither):

- `METROCALK_GATE_INJECT_SLOW_US=<µs>` — busy-spin that many µs per sample, simulating a
  catastrophic regression. (Needed because an uncached 5k path is only ~12 µs — *nowhere
  near* 16 ms; tripping a 16 ms gate genuinely requires an order-of-magnitude blowup,
  which is exactly what the gate exists to catch.)
- `METROCALK_GATE_BUDGET_US=<µs>` — override the budget (calibration / tightening).

Local proof:

```
# green: GATE PASS, exit 0
cargo run --release -p query-gate
# red: p99 ≈ 17 ms > 16 ms budget → ::error:: + exit 1
METROCALK_GATE_INJECT_SLOW_US=17000 cargo run --release -p query-gate
```

**CI red proof (done, then reverted).** Injecting `17000` on a throwaway branch
(`verify/m1.5-perf-gate-red`, since deleted) drove the runner's p99 to **17 060 µs** and
turned `perf-gate` **red** — `::error:: compat-query p99 17060.26 us EXCEEDS the 16000 us
budget` / exit 1 / job failed — while `matched` stayed at 211 (the query is still
correct, just slow). `main` never carried the injection. This is the same green-and-red
demonstration spike ③'s wasm-tripwire required.
