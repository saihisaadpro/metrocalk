# Flecs (flecs_ecs 0.2.2 / flecs C v4.1.2) spike results

seed = 0x4D4554524F434131 · safety locks (`flecs_safety_locks`): **OFF**


## Bench 1 — compatibility query at 5000 entities ((Provides,Health) without (BindsTo,*))

matched entities: 211 (of 5000); first cached eval (cold): 25.5 µs

| query | median | p99 | max |
|---|---|---|---|
| cached (warm, steady state) | 8.4 µs | 14.2 µs | 133.8 µs |
| uncached (re-evaluated each call) | 101.3 µs | 172.0 µs | 416.7 µs |

## Bench 2 — cached query latency under mutation (100 pair add/removes between iterations)

| query after 100 mutations | median | p99 | max |
|---|---|---|---|
| cached re-query | 24.5 µs | 35.9 µs | 47.8 µs |

## Bench 3 — wildcard traversal: every (BindsTo, *) edge (relationship visualizer)

edges traversed: 1999

| traversal | median | p99 | max |
|---|---|---|---|
| every BindsTo edge (cached) | 112.0 µs | 148.6 µs | 221.3 µs |

## Bench 4 — churn correctness: 1,000 entities created then destroyed

baseline 211 → +1000 created → 1211 → destroyed → 211
destruct 1000 entities: 64 µs

**zero stale results: PASS** (expected 211 == 211 after destroy)

## Bench 1 — compatibility query at 20000 entities ((Provides,Health) without (BindsTo,*))

matched entities: 830 (of 20000); first cached eval (cold): 114.1 µs

| query | median | p99 | max |
|---|---|---|---|
| cached (warm, steady state) | 35.0 µs | 62.2 µs | 166.0 µs |
| uncached (re-evaluated each call) | 924.2 µs | 1.08 ms | 1.32 ms |

## Bench 5 — memory at 20000 entities

RSS before scene: 7.7 MB · after: 288.5 MB · entities: 20000
approx bytes/entity (delta): 14723

peak RSS over whole process: 294.9 MB
