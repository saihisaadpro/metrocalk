# Flecs (flecs_ecs 0.2.2 / flecs C v4.1.2) spike results

seed = 0x4D4554524F434131 · safety locks (`flecs_safety_locks`): **OFF**


## Bench 1 — compatibility query at 5000 entities ((Provides,Health) without (BindsTo,*))

matched entities: 211 (of 5000); first cached eval (cold): 29.8 µs

| query | median | p99 | max |
|---|---|---|---|
| cached (warm, steady state) | 8.4 µs | 14.2 µs | 139.1 µs |
| uncached (re-evaluated each call) | 100.9 µs | 227.1 µs | 354.9 µs |

## Bench 2 — cached query latency under mutation (100 pair add/removes between iterations)

| query after 100 mutations | median | p99 | max |
|---|---|---|---|
| cached re-query | 24.7 µs | 30.0 µs | 80.4 µs |

## Bench 3 — wildcard traversal: every (BindsTo, *) edge (relationship visualizer)

edges traversed: 1999

| traversal | median | p99 | max |
|---|---|---|---|
| every BindsTo edge (cached) | 111.8 µs | 148.7 µs | 255.3 µs |

## Bench 4 — churn correctness: 1,000 entities created then destroyed

baseline 211 → +1000 created → 1211 → destroyed → 211
destruct 1000 entities: 47 µs

**zero stale results: PASS** (expected 211 == 211 after destroy)

## Bench 1 — compatibility query at 20000 entities ((Provides,Health) without (BindsTo,*))

matched entities: 830 (of 20000); first cached eval (cold): 132.6 µs

| query | median | p99 | max |
|---|---|---|---|
| cached (warm, steady state) | 37.1 µs | 50.5 µs | 109.8 µs |
| uncached (re-evaluated each call) | 918.0 µs | 1.02 ms | 1.14 ms |

## Bench 5 — memory at 20000 entities

RSS before scene: 7.7 MB · after: 288.3 MB · entities: 20000
approx bytes/entity (delta): 14712

peak RSS over whole process: 294.4 MB
