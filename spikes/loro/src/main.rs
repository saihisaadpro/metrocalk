//! M0 spike: Loro 1.x as the Metrocalk document layer (ADR-002 gate).
//! One command runs the whole suite serially and prints markdown results:
//!   cargo run --release
//! Quality bar: trustworthy measurements, not production code.

mod rng;
mod scene;
mod validate;

use loro::{ExportMode, LoroDoc, TreeParentId, UndoManager};
use rng::Rng;
use scene::{child_map, Scene};
use std::time::Instant;

/// Fixed RNG seed for the synthetic scene ("METROCA1"). Same seed => same scene everywhere.
const SEED: u64 = 0x4D45_5452_4F43_4131;
const N_ENTITIES: usize = 5_000;
const N_EDGES: usize = 2_000;

fn main() {
    println!("# Loro 1.13.1 spike results\n");
    println!("seed = 0x{SEED:016X}, scene = {N_ENTITIES} entities / {N_EDGES} edges\n");

    let (doc10k, f_at_5k) = bench1_mutations();
    bench3_sizes(&doc10k.doc, "10k mutations");
    bench4_time_travel(&doc10k, &f_at_5k);
    let doc100k = bench_extend_to_100k(doc10k);
    bench3_sizes(&doc100k.doc, "100k mutations");
    drop(doc100k);

    bench2_undo(1_000);
    bench2_undo(10_000);

    bench5_merge();

    println!("\npeak RSS over whole process run: {:.1} MB", peak_rss_mb());
}

// ---------- bench 1: 10k sequential mutations ----------

fn bench1_mutations() -> (Scene, loro::Frontiers) {
    println!("## Bench 1 — 10,000 sequential mutations (70% prop set / 10% reparent / 5% create / 5% delete / 5% bind add / 5% bind remove)\n");
    let rss0 = cur_rss_mb();
    let t = Instant::now();
    let mut s = Scene::generate(SEED, N_ENTITIES, N_EDGES, 1);
    let gen_time = t.elapsed();
    println!(
        "scene generation: {:.2} s, doc ops after gen: {}, RSS {:.1} MB (baseline before gen {:.1} MB)",
        gen_time.as_secs_f64(),
        s.doc.len_ops(),
        cur_rss_mb(),
        rss0
    );

    let mut rng = Rng::new(SEED ^ 0xB1);
    // warm-up: 500 unmeasured mutations
    for _ in 0..500 {
        s.run_mutation(&mut rng);
    }
    let mut lat_us: Vec<f64> = Vec::with_capacity(10_000);
    let mut f_at_5k = s.doc.oplog_frontiers();
    let wall = Instant::now();
    for i in 0..10_000 {
        let dt = s.run_mutation(&mut rng);
        lat_us.push(dt.as_secs_f64() * 1e6);
        if i == 4_999 {
            f_at_5k = s.doc.oplog_frontiers();
        }
    }
    let wall = wall.elapsed();
    let st = stats(&mut lat_us);
    println!("\n| metric | value |");
    println!("|---|---|");
    println!("| total wall time | {:.3} s |", wall.as_secs_f64());
    println!("| throughput | {:.0} mutations/s |", 10_000.0 / wall.as_secs_f64());
    println!("| per-op median | {:.1} µs |", st.median);
    println!("| per-op p99 | {:.1} µs |", st.p99);
    println!("| per-op max | {:.1} µs |", st.max);
    println!("| peak RSS (gen + 10.5k mutations) | {:.1} MB |", peak_rss_mb());
    println!(
        "| doc ops / changes after run | {} / {} |",
        s.doc.len_ops(),
        s.doc.len_changes()
    );
    println!("| entities / edges alive | {} / {} |", s.entities.len(), s.edges.len());
    (s, f_at_5k)
}

// ---------- bench 3: export sizes ----------

fn bench3_sizes(doc: &LoroDoc, label: &str) {
    println!("\n## Bench 3 — export sizes at {label} (doc ops: {})\n", doc.len_ops());
    let t = Instant::now();
    let full = doc.export(ExportMode::Snapshot).unwrap();
    let t_full = t.elapsed();
    let f = doc.oplog_frontiers();
    let t = Instant::now();
    let shallow = doc.export(ExportMode::shallow_snapshot(&f)).unwrap();
    let t_shallow = t.elapsed();
    let t = Instant::now();
    let oplog = doc.export(ExportMode::all_updates()).unwrap();
    let t_oplog = t.elapsed();
    println!("| export | size | time |");
    println!("|---|---|---|");
    println!("| full snapshot | {} | {:.0} ms |", fmt_size(full.len()), t_full.as_secs_f64() * 1e3);
    println!("| shallow snapshot (at latest) | {} | {:.0} ms |", fmt_size(shallow.len()), t_shallow.as_secs_f64() * 1e3);
    println!("| oplog (all updates) | {} | {:.0} ms |", fmt_size(oplog.len()), t_oplog.as_secs_f64() * 1e3);
}

// ---------- bench 4: time-travel ----------

fn bench4_time_travel(s: &Scene, f_at_5k: &loro::Frontiers) {
    println!("\n## Bench 4 — time-travel checkout to 5k mutations in the past (5 iterations)\n");
    println!("| iter | checkout back | checkout to latest |");
    println!("|---|---|---|");
    for i in 0..5 {
        let t = Instant::now();
        s.doc.checkout(f_at_5k).unwrap();
        let back = t.elapsed();
        let t = Instant::now();
        s.doc.checkout_to_latest();
        let fwd = t.elapsed();
        println!(
            "| {} | {:.1} ms | {:.1} ms |",
            i + 1,
            back.as_secs_f64() * 1e3,
            fwd.as_secs_f64() * 1e3
        );
    }
}

// ---------- extend history to 100k for the second size measurement ----------

fn bench_extend_to_100k(mut s: Scene) -> Scene {
    let mut rng = Rng::new(SEED ^ 0xC2);
    let t = Instant::now();
    for _ in 0..90_000 {
        s.run_mutation(&mut rng);
    }
    println!(
        "\n(extended history by 90k more mutations in {:.1} s — {:.0} mut/s sustained)",
        t.elapsed().as_secs_f64(),
        90_000.0 / t.elapsed().as_secs_f64()
    );
    s
}

// ---------- bench 2: undo/redo latency ----------

fn bench2_undo(depth: usize) {
    println!("\n## Bench 2 — undo/redo latency at history depth {depth}\n");
    let mut s = Scene::generate(SEED, N_ENTITIES, N_EDGES, 1);
    let mut undo = UndoManager::new(&s.doc);
    undo.set_max_undo_steps(50_000);
    undo.set_merge_interval(0);

    let mut rng = Rng::new(SEED ^ 0xD3);
    for _ in 0..depth {
        s.run_mutation(&mut rng);
    }

    // --- single-op undo/redo OF THE LATEST op (interactive Ctrl-Z) ---
    // Time only the undo, then redo (untimed) to return to the tip, so every measured undo
    // reverts the most-recent op (one step from tip) — the cost a user actually feels.
    for _ in 0..10 {
        assert!(undo.undo().unwrap());
        assert!(undo.redo().unwrap());
    }
    let mut undo_us = Vec::with_capacity(100);
    let mut redo_us = Vec::with_capacity(100);
    for _ in 0..100 {
        let t = Instant::now();
        assert!(undo.undo().unwrap());
        undo_us.push(t.elapsed().as_secs_f64() * 1e6);
        let t = Instant::now();
        assert!(undo.redo().unwrap());
        redo_us.push(t.elapsed().as_secs_f64() * 1e6);
    }
    let su = stats(&mut undo_us);
    let sr = stats(&mut redo_us);

    // --- consecutive undo (hold Ctrl-Z): 100 undos in a row, no redo between ---
    // Shows how cost grows as the checkout target moves away from the tip.
    let mut consec_us = Vec::with_capacity(100);
    for _ in 0..100 {
        let t = Instant::now();
        assert!(undo.undo().unwrap());
        consec_us.push(t.elapsed().as_secs_f64() * 1e6);
    }
    let sc = stats(&mut consec_us);
    for _ in 0..100 {
        assert!(undo.redo().unwrap());
    }

    // --- 50-op transaction group: undo/redo OF THE LATEST group ---
    let mut grng = Rng::new(SEED ^ 0xE4);
    for _ in 0..30 {
        undo.group_start().unwrap();
        for _ in 0..50 {
            s.run_mutation(&mut grng);
        }
        undo.group_end();
    }
    let mut gundo_us = Vec::with_capacity(30);
    let mut gredo_us = Vec::with_capacity(30);
    for _ in 0..30 {
        let t = Instant::now();
        assert!(undo.undo().unwrap());
        gundo_us.push(t.elapsed().as_secs_f64() * 1e6);
        let t = Instant::now();
        assert!(undo.redo().unwrap());
        gredo_us.push(t.elapsed().as_secs_f64() * 1e6);
    }
    let sgu = stats(&mut gundo_us);
    let sgr = stats(&mut gredo_us);

    println!("| op (n samples) | median | p99 | max |");
    println!("|---|---|---|---|");
    println!("| single undo of latest (100) | {:.0} µs | {:.0} µs | {:.0} µs |", su.median, su.p99, su.max);
    println!("| single redo of latest (100) | {:.0} µs | {:.0} µs | {:.0} µs |", sr.median, sr.p99, sr.max);
    println!("| consecutive undo, 1→100 back (100) | {:.0} µs | {:.0} µs | {:.0} µs |", sc.median, sc.p99, sc.max);
    println!("| 50-op group undo of latest (30) | {:.0} µs | {:.0} µs | {:.0} µs |", sgu.median, sgu.p99, sgu.max);
    println!("| 50-op group redo of latest (30) | {:.0} µs | {:.0} µs | {:.0} µs |", sgr.median, sgr.p99, sgr.max);
}

// ---------- bench 5: merge stress ----------

fn bench5_merge() {
    println!("\n## Bench 5 — merge stress (fork, 500 divergent ops/side + scripted conflicts, merge)\n");
    let base = Scene::generate(SEED, N_ENTITIES, N_EDGES, 1);

    // Deterministic entity picks for scripted conflicts.
    let xa_tid = base.entities[100].tid;
    let ya_tid = base.entities[200].tid;
    let del_e = base.entities[300].eid.clone();
    let del_tid = base.entities[300].tid;
    let edit_target = base.entities[400].eid.clone();
    let asset_target = base.entities[500].eid.clone();
    let bind_from = base.entities[600].eid.clone();
    let bind_to = base.entities[700].eid.clone();
    let reparent2 = base.entities[800].tid;
    let pa = base.entities[810].tid;
    let pb = base.entities[820].tid;
    let marker = base.entities[50].eid.clone();

    // Pre-create the containers that S5/S6 will edit on BOTH sides, so those conflicts exercise
    // field-level LWW on a *shared* container (same cid in the common ancestor) rather than the
    // concurrent-container-creation case (which S7 covers).
    child_map(&child_map(&base.components, &edit_target), "Health").insert("hp", 0i64).unwrap();
    child_map(&child_map(&base.components, &asset_target), "MeshRenderer").insert("mesh", "assets/asset_seed.glb").unwrap();
    base.doc.commit();
    let base_vv = base.doc.oplog_vv();

    let mut a = base; // peer 1
    let forked = a.doc.fork();
    forked.set_peer_id(2).unwrap();
    let mut b = a.rebind(forked);

    // S1 conflicting reparents: A moves X under Y, B moves Y under X
    a.tree.mov(xa_tid, ya_tid).unwrap();
    b.tree.mov(ya_tid, xa_tid).unwrap();
    // S2 delete-vs-edit: A deletes E (incl. its components record), B edits E's component
    a.tree.delete(del_tid).unwrap();
    a.components.delete(&del_e).unwrap();
    child_map(&child_map(&b.components, &del_e), "Transform").insert("px", 123.456).unwrap();
    // S3 delete-vs-create-child: B creates a child under the entity A deleted
    let orphan_child = b.tree.create(TreeParentId::Node(del_tid)).unwrap();
    b.tree.get_meta(orphan_child).unwrap().insert("eid", "e_orphan_child").unwrap();
    child_map(&b.components, "e_orphan_child");
    // S4 delete-vs-bind: B adds a binding referencing the deleted entity
    let dang_key = format!("{}|observes|{}", edit_target, del_e);
    let em = child_map(&b.bindings, &dang_key);
    em.insert("from", edit_target.as_str()).unwrap();
    em.insert("to", del_e.as_str()).unwrap();
    em.insert("kind", "observes").unwrap();
    // S5 same-field conflict (LWW check) on a shared container: both set the same field differently
    child_map(&child_map(&a.components, &edit_target), "Health").insert("hp", 111i64).unwrap();
    child_map(&child_map(&b.components, &edit_target), "Health").insert("hp", 222i64).unwrap();
    // S6 same asset-ref field set to different paths on both sides (shared container)
    child_map(&child_map(&a.components, &asset_target), "MeshRenderer").insert("mesh", "assets/asset_aaaa.glb").unwrap();
    child_map(&child_map(&b.components, &asset_target), "MeshRenderer").insert("mesh", "assets/asset_bbbb.glb").unwrap();
    // S7 same edge key created concurrently on both sides (NOT pre-created → two regular containers)
    let same_key = format!("{}|bindsTo|{}", bind_from, bind_to);
    for sc in [&a, &b] {
        let em = child_map(&sc.bindings, &same_key);
        em.insert("from", bind_from.as_str()).unwrap();
        em.insert("to", bind_to.as_str()).unwrap();
        em.insert("kind", "bindsTo").unwrap();
    }
    // S8 concurrent reparent of the same node to two different parents
    a.tree.mov(reparent2, pa).unwrap();
    b.tree.mov(reparent2, pb).unwrap();
    a.doc.commit();
    b.doc.commit();

    // ---- undo manager on A, created before A's random divergence ----
    let mut a_undo = UndoManager::new(&a.doc);
    a_undo.set_max_undo_steps(50_000);
    a_undo.set_merge_interval(0);

    // ---- 500 random divergent mutations per side ----
    let mut rng_a = Rng::new(SEED ^ 0xAA);
    let mut rng_b = Rng::new(SEED ^ 0xBB);
    for _ in 0..500 {
        a.run_mutation(&mut rng_a);
        b.run_mutation(&mut rng_b);
    }
    // marker op: A sets a known field last, so we can verify local undo after merge
    child_map(&child_map(&a.components, &marker), "Transform").insert("px", 999.0).unwrap();
    a.doc.commit();

    // ---- merge: exchange updates both ways ----
    let t = Instant::now();
    let b_updates = b.doc.export(ExportMode::updates(&base_vv)).unwrap();
    let st_a = a.doc.import(&b_updates).unwrap();
    let t_import_a = t.elapsed();
    assert!(st_a.pending.is_none(), "pending deps on import into A");
    let t = Instant::now();
    let a_updates = a.doc.export(ExportMode::updates(&base_vv)).unwrap();
    let st_b = b.doc.import(&a_updates).unwrap();
    let t_import_b = t.elapsed();
    assert!(st_b.pending.is_none(), "pending deps on import into B");

    let ca = validate::canon_doc(&a.doc);
    let cb = validate::canon_doc(&b.doc);
    println!("| merge metric | value |");
    println!("|---|---|");
    println!("| update payload B→A | {} |", fmt_size(b_updates.len()));
    println!("| B→A export+import | {:.1} ms |", t_import_a.as_secs_f64() * 1e3);
    println!("| A→B export+import | {:.1} ms |", t_import_b.as_secs_f64() * 1e3);
    println!("| converged (canonical deep-value equal) | {} |", ca == cb);

    // ---- scripted-conflict outcomes ----
    println!("\n### Scripted conflict outcomes\n");
    let p_x = a.tree.parent(xa_tid);
    let p_y = a.tree.parent(ya_tid);
    println!("- S1 conflicting reparents (X↔Y): X.parent={p_x:?}, Y.parent={p_y:?} (X={xa_tid:?}, Y={ya_tid:?})");
    let p_r = a.tree.parent(reparent2);
    println!("- S8 same node to two parents: parent={p_r:?} (A chose {pa:?}, B chose {pb:?})");
    let s2 = a.components.get(&del_e).is_some();
    let s2_node_deleted = a.tree.is_node_deleted(&del_tid).unwrap();
    println!("- S2 delete-vs-edit: node deleted={s2_node_deleted}, component record resurrected={s2}");
    let s3_parent = a.tree.parent(orphan_child);
    let s3_deleted = a.tree.is_node_deleted(&orphan_child).unwrap();
    println!("- S3 create-under-deleted: child parent={s3_parent:?}, child deleted={s3_deleted}");
    let s4 = a.bindings.get(&dang_key).is_some();
    println!("- S4 delete-vs-bind: dangling edge present={s4}");
    let hp = deep_field(&a.doc, &edit_target, "Health", "hp");
    println!("- S5 same-field LWW: hp={hp} (A wrote 111, B wrote 222 — one whole value wins)");
    let mesh = deep_field(&a.doc, &asset_target, "MeshRenderer", "mesh");
    println!("- S6 asset-ref LWW: mesh={mesh}");
    let s7 = deep_edge_count(&a.doc, &same_key);
    println!("- S7 same-edge concurrent add: entries with that key={s7} (map key dedup)");

    // ---- undo after merge: does local-only undo hold? ----
    println!("\n### Undo after merge (A's UndoManager)\n");
    let before = deep_field(&a.doc, &marker, "Transform", "px");
    let hp_before_undo = deep_field(&a.doc, &edit_target, "Health", "hp");
    let t = Instant::now();
    let did = a_undo.undo().unwrap();
    let undo_t = t.elapsed();
    let after = deep_field(&a.doc, &marker, "Transform", "px");
    println!("- first undo after merge: performed={did}, took {:.1} ms, marker px {before} -> {after} (expected 999 -> previous)", undo_t.as_secs_f64() * 1e3);
    let s2_after_first_undo = a.components.get(&del_e).is_some();
    println!("- B-authored resurrected record still present after A's undo: {s2_after_first_undo}");
    let t = Instant::now();
    let mut n_undone = 1;
    while a_undo.undo().unwrap() {
        n_undone += 1;
        if n_undone > 2_000 {
            break;
        }
    }
    println!(
        "- unwound {} local undo steps in {:.1} ms total; B's hp value after full unwind: {} (was {} post-merge)",
        n_undone,
        t.elapsed().as_secs_f64() * 1e3,
        deep_field(&a.doc, &edit_target, "Health", "hp"),
        hp_before_undo
    );
    while a_undo.redo().unwrap() {}

    // ---- invalid-state catalog ----
    println!("\n### Post-merge validation (doc A)\n");
    let rep = validate::validate(&a.doc);
    println!(
        "alive nodes: {}, component records: {}, edges: {}, total violations: {}\n",
        rep.alive_nodes,
        rep.component_records,
        rep.edges,
        rep.total()
    );
    println!("| violation class | count | examples |");
    println!("|---|---|---|");
    for (class, items) in &rep.violations {
        let ex: Vec<&str> = items.iter().take(3).map(|s| s.as_str()).collect();
        println!("| {class} | {} | {} |", items.len(), ex.join(" · "));
    }
    if rep.violations.is_empty() {
        println!("| (none) | 0 | |");
    }
}

fn deep_field(doc: &LoroDoc, eid: &str, comp: &str, field: &str) -> String {
    let v = doc.get_map("components").get_deep_value();
    if let loro::LoroValue::Map(m) = v {
        if let Some(loro::LoroValue::Map(rec)) = m.get(eid) {
            if let Some(loro::LoroValue::Map(c)) = rec.get(comp) {
                if let Some(f) = c.get(field) {
                    return format!("{f:?}");
                }
            }
        }
    }
    "<missing>".into()
}

fn deep_edge_count(doc: &LoroDoc, key: &str) -> usize {
    let v = doc.get_map("bindings").get_deep_value();
    if let loro::LoroValue::Map(m) = v {
        m.iter().filter(|(k, _)| k.as_str() == key).count()
    } else {
        0
    }
}

// ---------- helpers ----------

struct Stats {
    median: f64,
    p99: f64,
    max: f64,
}

fn stats(xs: &mut [f64]) -> Stats {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = xs.len();
    Stats {
        median: xs[n / 2],
        p99: xs[((n as f64 * 0.99).ceil() as usize).min(n) - 1],
        max: xs[n - 1],
    }
}

fn fmt_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

fn mem_counters() -> windows_sys::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb);
        pmc
    }
}

fn peak_rss_mb() -> f64 {
    mem_counters().PeakWorkingSetSize as f64 / (1024.0 * 1024.0)
}

fn cur_rss_mb() -> f64 {
    mem_counters().WorkingSetSize as f64 / (1024.0 * 1024.0)
}
