//! Post-merge validation and repair layer (invariant 3, second sentence).
//!
//! After importing/merging Loro updates, this module detects and repairs the eight invalid-state
//! classes catalogued in `spikes/loro` (F1). Uses regular containers per F1 (the `ensure_mergeable_*`
//! helper breaks under undo/redo).
//!
//! ## Adversarial analysis: in-memory inverse-op stack vs Loro op history
//!
//! **The case:** a merge lands between an op and its undo. Peer A commits op X (health 50→100).
//! The inverse-op stack records "set health back to 50." Peer B's merge arrives, setting health
//! to 200 via LWW. User undoes X: the engine applies "set health to 50," overriding B's value.
//!
//! **Why this is correct for M1:** Undo means "reverse MY action." The inverse op is applied as a
//! new forward commit to both ECS and Loro, so they stay consistent. Loro sees A's original op,
//! B's merge, and A's undo — three distinct operations. A future re-merge with B reconciles via
//! CRDT semantics. If the resulting state violates scene invariants, the merge-validation layer
//! catches and repairs it. This is operational undo (apply inverse), not history rewind — the
//! standard model for collaborative editing without operational transformation.
//!
//! **What could break:** if undo is expected to be "selective" (undo A's change while preserving
//! B's concurrent change to the same field), the current design does not support that. Selective
//! undo requires operational transformation, which is Phase-2 scope. For M1, we clear the undo
//! stack on merge to sidestep this entirely — the adversarial case cannot arise.

use crate::pipeline::child_map;
use loro::{LoroDoc, LoroValue, TreeParentId};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

/// Result of merge validation: violations found, repairs applied.
#[derive(Debug, Default)]
pub struct MergeReport {
    pub alive_nodes: usize,
    pub component_records: usize,
    pub edges: usize,
    pub violations: BTreeMap<&'static str, Vec<String>>,
    pub total_repairs: usize,
}

impl MergeReport {
    pub fn total_violations(&self) -> usize {
        self.violations.values().map(Vec::len).sum()
    }

    fn add(&mut self, class: &'static str, detail: String) {
        self.violations.entry(class).or_default().push(detail);
    }
}

/// Validate the Loro document against the 8 invalid-state classes, repair each violation,
/// and return a report.
pub(crate) fn validate_and_repair(doc: &LoroDoc) -> MergeReport {
    let mut report = MergeReport::default();
    detect(doc, &mut report);
    report.total_repairs = repair(doc, &report);
    report
}

#[allow(clippy::too_many_lines)] // one linear scan over the 8 invalid-state classes; splitting it would scatter shared state
fn detect(doc: &LoroDoc, rep: &mut MergeReport) {
    const ASSET_FIELDS: &[&str] = &[
        "mesh",
        "material",
        "texture",
        "source",
        "clip",
        "controller",
    ];
    const ASSET_EXTS: &[&str] = &["glb", "png", "wav", "wgsl"];

    let tree = doc.get_tree("hierarchy");
    let components = doc.get_map("components");
    let bindings = doc.get_map("bindings");

    // ── 1. Collect alive nodes and their eids ──────────────────────────

    let all_nodes = tree.nodes();
    let mut alive_eids: HashSet<String> = HashSet::new();
    let mut alive_count = 0usize;

    for tid in &all_nodes {
        let Ok(false) = tree.is_node_deleted(tid) else {
            continue;
        };
        alive_count += 1;

        let Ok(meta) = tree.get_meta(*tid) else {
            continue;
        };
        let eid = meta
            .get("eid")
            .and_then(|v| v.as_value().cloned())
            .and_then(|v| match v {
                LoroValue::String(s) => Some(s.to_string()),
                _ => None,
            });
        if let Some(eid) = eid {
            if !alive_eids.insert(eid.clone()) {
                // CLASS 4: duplicate eid
                rep.add("duplicate-eid", eid);
            }
        }
    }
    rep.alive_nodes = alive_count;

    // ── 2. Tree cycle detection ────────────────────────────────────────

    let cap = all_nodes.len() + 2;
    for tid in &all_nodes {
        let Ok(false) = tree.is_node_deleted(tid) else {
            continue;
        };
        let mut cur = *tid;
        let mut steps = 0;
        loop {
            match tree.parent(cur) {
                Some(TreeParentId::Node(p)) => {
                    steps += 1;
                    if steps > cap {
                        // CLASS 5: tree cycle
                        rep.add("tree-cycle", format!("{tid:?}"));
                        break;
                    }
                    cur = p;
                }
                Some(TreeParentId::Root) | None => break,
                Some(TreeParentId::Deleted | TreeParentId::Unexist) => {
                    // CLASS 6: alive node under deleted ancestor
                    rep.add("alive-under-deleted-ancestor", format!("{tid:?}"));
                    break;
                }
            }
        }
    }

    // ── 3. Component ↔ tree consistency ────────────────────────────────

    let comp_keys: Vec<String> = components.keys().map(|k| k.to_string()).collect();
    rep.component_records = comp_keys.len();

    for eid in &comp_keys {
        if !alive_eids.contains(eid) {
            // CLASS 2: orphan component record
            rep.add("orphan-component-record", eid.clone());
        }
    }
    let comp_keyset: HashSet<&String> = comp_keys.iter().collect();
    for eid in &alive_eids {
        if !comp_keyset.contains(eid) {
            // CLASS 3: entity missing component record
            rep.add("entity-missing-component-record", eid.clone());
        }
    }

    // ── 4. Asset-reference integrity ───────────────────────────────────

    if let LoroValue::Map(m) = components.get_deep_value() {
        for (eid, rec) in m.iter() {
            let LoroValue::Map(comps) = rec else {
                continue;
            };
            for (cname, cval) in comps.iter() {
                let LoroValue::Map(fields) = cval else {
                    continue;
                };
                for (fname, fval) in fields.iter() {
                    if !ASSET_FIELDS.contains(&fname.as_str()) {
                        continue;
                    }
                    let ok = match fval {
                        LoroValue::String(s) => {
                            // A content-addressed store handle (ADR-014, `mtkasset:<hex>` — the canonical
                            // asset identity `place_mesh`/the blobstore/CSG produce) OR a legacy
                            // `assets/<name>.<ext>` path. Before this, a content-addressed mesh was flagged
                            // corrupt on every merge/reload (a project saved with a generated/imported mesh
                            // would not round-trip clean, and an ECO gating on 0 violations would reject it).
                            s.starts_with("mtkasset:")
                                || (s.starts_with("assets/")
                                    && ASSET_EXTS.iter().any(|e| s.ends_with(&format!(".{e}"))))
                        }
                        _ => false,
                    };
                    if !ok {
                        // CLASS 7: corrupt asset ref
                        rep.add(
                            "corrupt-asset-ref",
                            format!("{eid}.{cname}.{fname} = {fval:?}"),
                        );
                    }
                }
            }
        }
    }

    // ── 5. Binding integrity ───────────────────────────────────────────

    if let LoroValue::Map(m) = bindings.get_deep_value() {
        rep.edges = m.len();
        for (key, val) in m.iter() {
            let LoroValue::Map(edge) = val else {
                // CLASS 8: malformed edge
                rep.add("malformed-edge", key.clone());
                continue;
            };
            let from = edge.get("from").and_then(loro_str);
            let to = edge.get("to").and_then(loro_str);
            let kind = edge.get("kind").and_then(loro_str);
            let (Some(from), Some(to), Some(_kind)) = (from, to, kind) else {
                // CLASS 8: malformed edge (missing fields)
                rep.add("malformed-edge", format!("{key}: missing from/to/kind"));
                continue;
            };
            // CLASS 1: dangling edge endpoint
            if !alive_eids.contains(&from) {
                rep.add(
                    "dangling-edge-endpoint",
                    format!("{key} (from {from} dead)"),
                );
            }
            if !alive_eids.contains(&to) {
                rep.add("dangling-edge-endpoint", format!("{key} (to {to} dead)"));
            }
        }
    }
}

/// Apply repairs for detected violations. Returns the number of repairs made.
fn repair(doc: &LoroDoc, report: &MergeReport) -> usize {
    let mut repairs = 0;

    // Repair orphan component records: delete them
    if let Some(orphans) = report.violations.get("orphan-component-record") {
        let components = doc.get_map("components");
        for eid in orphans {
            if components.get(eid).is_some() {
                components.delete(eid).unwrap();
                repairs += 1;
            }
        }
    }

    // Repair entity-missing-component-record: create empty record
    if let Some(missing) = report.violations.get("entity-missing-component-record") {
        let components = doc.get_map("components");
        for eid in missing {
            let _ = child_map(&components, eid);
            repairs += 1;
        }
    }

    // Repair dangling edge endpoints: delete the edge
    if let Some(dangles) = report.violations.get("dangling-edge-endpoint") {
        let bindings = doc.get_map("bindings");
        let mut deleted: HashSet<String> = HashSet::new();
        for detail in dangles {
            // Extract edge key from detail format "key (from/to eid dead)"
            let key = detail.split(" (").next().unwrap_or(detail);
            if deleted.insert(key.to_string()) && bindings.get(key).is_some() {
                bindings.delete(key).unwrap();
                repairs += 1;
            }
        }
    }

    // Repair malformed edges: delete them
    if let Some(malformed) = report.violations.get("malformed-edge") {
        let bindings = doc.get_map("bindings");
        for detail in malformed {
            let key = detail.split(':').next().unwrap_or(detail);
            if bindings.get(key).is_some() {
                bindings.delete(key).unwrap();
                repairs += 1;
            }
        }
    }

    // Repair duplicate eids: re-key the duplicate by appending a disambiguator
    if let Some(dups) = report.violations.get("duplicate-eid") {
        let tree = doc.get_tree("hierarchy");
        let all_nodes = tree.nodes();
        for dup_eid in dups {
            // Find alive nodes with this eid (there are ≥2); re-key all but the first
            let mut found_first = false;
            for tid in &all_nodes {
                let Ok(false) = tree.is_node_deleted(tid) else {
                    continue;
                };
                let Ok(meta) = tree.get_meta(*tid) else {
                    continue;
                };
                let eid = meta
                    .get("eid")
                    .and_then(|v| v.as_value().cloned())
                    .and_then(|v| match v {
                        LoroValue::String(s) => Some(s.to_string()),
                        _ => None,
                    });
                if eid.as_deref() == Some(dup_eid.as_str()) {
                    if found_first {
                        // Re-key using Loro's own TreeID (globally unique)
                        let new_eid = format!("{:x}_{:x}", tid.peer, tid.counter);
                        meta.insert("eid", new_eid.as_str()).unwrap();
                        // Move the component record too
                        let components = doc.get_map("components");
                        if let Some(loro::ValueOrContainer::Container(loro::Container::Map(
                            old_rec,
                        ))) = components.get(dup_eid)
                        {
                            let new_rec = child_map(&components, &new_eid);
                            if let LoroValue::Map(fields) = old_rec.get_deep_value() {
                                for (k, v) in fields.iter() {
                                    if let LoroValue::Map(comp_fields) = v {
                                        let comp_map = child_map(&new_rec, k);
                                        for (fk, fv) in comp_fields.iter() {
                                            comp_map.insert(fk, fv.clone()).unwrap();
                                        }
                                    }
                                }
                            }
                        }
                        repairs += 1;
                    }
                    found_first = true;
                }
            }
        }
    }

    // Corrupt asset refs: flag only (no automatic repair — the correct value is unknown).
    // The report carries the violation for the caller to present to the user.

    // Tree cycles: MovableTree CRDT prevents them (0 observed in spike). If found, no automated
    // repair (would require domain knowledge about which edge to break).

    // Alive-under-deleted-ancestor: no repair (the node IS effectively deleted; the validator
    // just flags it). The subtree is unreachable.

    repairs
}

fn loro_str(v: &LoroValue) -> Option<String> {
    match v {
        LoroValue::String(s) => Some(s.to_string()),
        _ => None,
    }
}

/// Deterministic deep-value serialization for convergence comparison.
pub fn canon_doc(doc: &LoroDoc) -> String {
    let mut s = String::new();
    canon(&doc.get_deep_value(), &mut s);
    s
}

fn canon(v: &LoroValue, out: &mut String) {
    match v {
        LoroValue::Null => out.push_str("null"),
        LoroValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        LoroValue::Double(d) => write!(out, "{d:?}").unwrap(),
        LoroValue::I64(i) => out.push_str(&i.to_string()),
        LoroValue::Binary(b) => write!(out, "bin[{}]", b.len()).unwrap(),
        LoroValue::String(s) => {
            out.push('"');
            out.push_str(s.as_str());
            out.push('"');
        }
        LoroValue::List(l) => {
            out.push('[');
            for x in l.iter() {
                canon(x, out);
                out.push(',');
            }
            out.push(']');
        }
        LoroValue::Map(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for k in keys {
                out.push_str(k);
                out.push(':');
                canon(&m[k], out);
                out.push(',');
            }
            out.push('}');
        }
        LoroValue::Container(c) => write!(out, "container({c})").unwrap(),
    }
}
