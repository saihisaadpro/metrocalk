# Flecs (flecs_ecs 0.2.2 / flecs C v4.1.2) spike results

seed = 0x4D4554524F434131 · safety locks (`flecs_safety_locks`): **ON**


## Bench 1 — compatibility query at 5000 entities ((Provides,Health) without (BindsTo,*))

matched entities: 211 (of 5000); first cached eval (cold): 29.0 µs

| query | median | p99 | max |
|---|---|---|---|
| cached (warm, steady state) | 8.7 µs | 12.2 µs | 24.7 µs |
| uncached (re-evaluated each call) | 107.8 µs | 124.4 µs | 235.6 µs |

## Bench 2 — cached query latency under mutation (100 pair add/removes between iterations)

| query after 100 mutations | median | p99 | max |
|---|---|---|---|
| cached re-query | 25.0 µs | 41.1 µs | 55.3 µs |

## Bench 3 — wildcard traversal: every (BindsTo, *) edge (relationship visualizer)

edges traversed: 1999

| traversal | median | p99 | max |
|---|---|---|---|
| every BindsTo edge (cached) | 129.4 µs | 166.1 µs | 211.1 µs |

## Bench 4 — churn correctness: 1,000 entities created then destroyed

baseline 211 → +1000 created → 1211 → destroyed → 211
destruct 1000 entities: 46 µs

**zero stale results: PASS** (expected 211 == 211 after destroy)

## Bench 1 — compatibility query at 20000 entities ((Provides,Health) without (BindsTo,*))

matched entities: 830 (of 20000); first cached eval (cold): 131.9 µs

| query | median | p99 | max |
|---|---|---|---|
| cached (warm, steady state) | 39.8 µs | 58.3 µs | 140.4 µs |
| uncached (re-evaluated each call) | 908.3 µs | 1.04 ms | 1.51 ms |

## Bench 5 — memory at 20000 entities

RSS before scene: 7.7 MB · after: 288.9 MB · entities: 20000
approx bytes/entity (delta): 14744

peak RSS over whole process: 295.4 MB
