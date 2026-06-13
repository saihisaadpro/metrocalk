# Flecs (flecs_ecs 0.2.2 / flecs C v4.1.2) spike results

seed = 0x4D4554524F434131 · safety locks (`flecs_safety_locks`): **ON**


## Bench 1 — compatibility query at 5000 entities ((Provides,Health) without (BindsTo,*))

matched entities: 211 (of 5000); first cached eval (cold): 29.1 µs

| query | median | p99 | max |
|---|---|---|---|
| cached (warm, steady state) | 9.6 µs | 12.3 µs | 34.4 µs |
| uncached (re-evaluated each call) | 105.8 µs | 128.1 µs | 160.2 µs |

## Bench 2 — cached query latency under mutation (100 pair add/removes between iterations)

| query after 100 mutations | median | p99 | max |
|---|---|---|---|
| cached re-query | 24.5 µs | 41.3 µs | 48.3 µs |

## Bench 3 — wildcard traversal: every (BindsTo, *) edge (relationship visualizer)

edges traversed: 1999

| traversal | median | p99 | max |
|---|---|---|---|
| every BindsTo edge (cached) | 131.0 µs | 152.9 µs | 212.3 µs |

## Bench 4 — churn correctness: 1,000 entities created then destroyed

baseline 211 → +1000 created → 1211 → destroyed → 211
destruct 1000 entities: 46 µs

**zero stale results: PASS** (expected 211 == 211 after destroy)

## Bench 1 — compatibility query at 20000 entities ((Provides,Health) without (BindsTo,*))

matched entities: 830 (of 20000); first cached eval (cold): 126.8 µs

| query | median | p99 | max |
|---|---|---|---|
| cached (warm, steady state) | 40.4 µs | 61.2 µs | 166.1 µs |
| uncached (re-evaluated each call) | 930.9 µs | 1.06 ms | 1.20 ms |

## Bench 5 — memory at 20000 entities

RSS before scene: 8.0 MB · after: 290.0 MB · entities: 20000
approx bytes/entity (delta): 14786

peak RSS over whole process: 296.4 MB
