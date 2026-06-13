# Loro 1.13.1 spike results

seed = 0x4D4554524F434131, scene = 5000 entities / 2000 edges

## Bench 1 — 10,000 sequential mutations (70% prop set / 10% reparent / 5% create / 5% delete / 5% bind add / 5% bind remove)

scene generation: 0.72 s, doc ops after gen: 157826, RSS 52.1 MB (baseline before gen 4.5 MB)

| metric | value |
|---|---|
| total wall time | 0.563 s |
| throughput | 17756 mutations/s |
| per-op median | 12.7 µs |
| per-op p99 | 191.7 µs |
| per-op max | 1414.4 µs |
| peak RSS (gen + 10.5k mutations) | 56.2 MB |
| doc ops / changes after run | 186528 / 153 |
| entities / edges alive | 3792 / 978 |

## Bench 3 — export sizes at 10k mutations (doc ops: 186528)

| export | size | time |
|---|---|---|
| full snapshot | 4.51 MB | 537 ms |
| shallow snapshot (at latest) | 2.01 MB | 339 ms |
| oplog (all updates) | 2.29 MB | 308 ms |

## Bench 4 — time-travel checkout to 5k mutations in the past (5 iterations)

| iter | checkout back | checkout to latest |
|---|---|---|
| 1 | 205.6 ms | 100.3 ms |
| 2 | 94.1 ms | 120.4 ms |
| 3 | 131.4 ms | 194.3 ms |
| 4 | 141.9 ms | 119.1 ms |
| 5 | 124.1 ms | 104.4 ms |

(extended history by 90k more mutations in 4.8 s — 18914 mut/s sustained)

## Bench 3 — export sizes at 100k mutations (doc ops: 422883)

| export | size | time |
|---|---|---|
| full snapshot | 9.26 MB | 1085 ms |
| shallow snapshot (at latest) | 218.2 KB | 652 ms |
| oplog (all updates) | 5.38 MB | 1026 ms |

## Bench 2 — undo/redo latency at history depth 1000

| op (n samples) | median | p99 | max |
|---|---|---|---|
| single undo of latest (100) | 70 µs | 523 µs | 610 µs |
| single redo of latest (100) | 70 µs | 642 µs | 2158 µs |
| consecutive undo, 1→100 back (100) | 52658 µs | 148480 µs | 161622 µs |
| 50-op group undo of latest (30) | 50272 µs | 82890 µs | 82890 µs |
| 50-op group redo of latest (30) | 50313 µs | 96352 µs | 96352 µs |

## Bench 2 — undo/redo latency at history depth 10000

| op (n samples) | median | p99 | max |
|---|---|---|---|
| single undo of latest (100) | 72 µs | 355 µs | 12015 µs |
| single redo of latest (100) | 72 µs | 1281 µs | 5340 µs |
| consecutive undo, 1→100 back (100) | 62083 µs | 136442 µs | 265416 µs |
| 50-op group undo of latest (30) | 62472 µs | 132064 µs | 132064 µs |
| 50-op group redo of latest (30) | 62322 µs | 239551 µs | 239551 µs |

## Bench 5 — merge stress (fork, 500 divergent ops/side + scripted conflicts, merge)

| merge metric | value |
|---|---|
| update payload B→A | 20.7 KB |
| B→A export+import | 400.1 ms |
| A→B export+import | 315.3 ms |
| converged (canonical deep-value equal) | true |

### Scripted conflict outcomes

- S1 conflicting reparents (X↔Y): X.parent=Some(Node(TreeID { peer: 1, counter: 6059 })), Y.parent=Some(Node(TreeID { peer: 1, counter: 1557 })) (X=TreeID { peer: 1, counter: 3066 }, Y=TreeID { peer: 1, counter: 6059 })
- S8 same node to two parents: parent=Some(Node(TreeID { peer: 1, counter: 24451 })) (A chose TreeID { peer: 1, counter: 24172 }, B chose TreeID { peer: 1, counter: 24451 })
- S2 delete-vs-edit: node deleted=true, component record resurrected=false
- S3 create-under-deleted: child parent=Some(Node(TreeID { peer: 1, counter: 8895 })), child deleted=true
- S4 delete-vs-bind: dangling edge present=true
- S5 same-field LWW: hp=I64(222) (A wrote 111, B wrote 222 — one whole value wins)
- S6 asset-ref LWW: mesh=String(LoroStringValue("assets/asset_bbbb.glb"))
- S7 same-edge concurrent add: entries with that key=1 (map key dedup)

### Undo after merge (A's UndoManager)

- first undo after merge: performed=true, took 94.2 ms, marker px Double(999.0) -> Double(67.02896389460096) (expected 999 -> previous)
- B-authored resurrected record still present after A's undo: false
- unwound 465 local undo steps in 51581.8 ms total; B's hp value after full unwind: I64(222) (was I64(222) post-merge)

### Post-merge validation (doc A)

alive nodes: 4893, component records: 4886, edges: 1899, total violations: 58

| violation class | count | examples |
|---|---|---|
| dangling-edge-endpoint | 17 | e004762|bindsTo|e002945 (to e002945 dead) · e001779|bindsTo|e000237 (from e001779 dead) · e002058|follows|e002342 (from e002058 dead) |
| duplicate-eid | 23 | e005024 · e005005 · e005001 |
| entity-missing-component-record | 1 | e005002 |
| orphan-component-record | 17 | e000924 · e001245 · e001359 |

peak RSS over whole process run: 222.1 MB
