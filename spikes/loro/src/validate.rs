//! Post-merge invalid-state detector. Works purely from the document (no shadow model),
//! because after a merge the shadow is stale by definition. Every violation class found
//! here feeds the M1 merge-validation layer design.

use loro::{LoroDoc, LoroValue, TreeParentId};
use std::collections::{BTreeMap, HashSet};

pub struct ValidationReport {
    pub alive_nodes: usize,
    pub component_records: usize,
    pub edges: usize,
    /// class -> example instances (capped)
    pub violations: BTreeMap<&'static str, Vec<String>>,
}

impl ValidationReport {
    pub fn total(&self) -> usize {
        self.violations.values().map(|v| v.len()).sum()
    }
    fn add(&mut self, class: &'static str, detail: String) {
        self.violations.entry(class).or_default().push(detail);
    }
}

fn vstr(v: &LoroValue) -> Option<String> {
    if let LoroValue::String(s) = v {
        Some(s.to_string())
    } else {
        None
    }
}

pub fn validate(doc: &LoroDoc) -> ValidationReport {
    let tree = doc.get_tree("hierarchy");
    let components = doc.get_map("components");
    let bindings = doc.get_map("bindings");

    let mut rep = ValidationReport {
        alive_nodes: 0,
        component_records: 0,
        edges: 0,
        violations: BTreeMap::new(),
    };

    // --- tree: alive nodes, eids, cycles ---
    let all_nodes = tree.nodes();
    let mut alive = Vec::new();
    for tid in &all_nodes {
        match tree.is_node_deleted(tid) {
            Ok(false) => alive.push(*tid),
            Ok(true) => {}
            Err(e) => rep.add("tree-query-error", format!("{tid:?}: {e}")),
        }
    }
    rep.alive_nodes = alive.len();

    let mut eid_of = std::collections::HashMap::new();
    let mut eids_alive: HashSet<String> = HashSet::new();
    for tid in &alive {
        let meta = match tree.get_meta(*tid) {
            Ok(m) => m,
            Err(e) => {
                rep.add("node-meta-error", format!("{tid:?}: {e}"));
                continue;
            }
        };
        match meta.get("eid").and_then(|v| v.as_value().cloned()).and_then(|v| vstr(&v)) {
            Some(eid) => {
                if !eids_alive.insert(eid.clone()) {
                    rep.add("duplicate-eid", eid.clone());
                }
                eid_of.insert(*tid, eid);
            }
            None => rep.add("node-missing-eid", format!("{tid:?}")),
        }
    }

    // cycle detection: walk parent chain with a step cap
    let cap = all_nodes.len() + 2;
    for tid in &alive {
        let mut cur = *tid;
        let mut steps = 0;
        loop {
            match tree.parent(cur) {
                Some(TreeParentId::Node(p)) => {
                    steps += 1;
                    if steps > cap {
                        rep.add("tree-cycle", format!("{tid:?}"));
                        break;
                    }
                    cur = p;
                }
                Some(TreeParentId::Root) | None => break,
                Some(TreeParentId::Deleted) | Some(TreeParentId::Unexist) => {
                    // alive node whose ancestor chain ends in trash — orphaned subtree
                    rep.add("alive-node-under-deleted-ancestor", format!("{tid:?}"));
                    break;
                }
            }
        }
    }

    // --- components <-> tree consistency ---
    let comp_keys: Vec<String> = components.keys().map(|k| k.to_string()).collect();
    rep.component_records = comp_keys.len();
    for eid in &comp_keys {
        if !eids_alive.contains(eid) {
            rep.add("orphan-component-record", eid.clone());
        }
    }
    let comp_keyset: HashSet<&String> = comp_keys.iter().collect();
    for eid in &eids_alive {
        if !comp_keyset.contains(eid) {
            rep.add("entity-missing-component-record", eid.clone());
        }
    }

    // --- asset-reference integrity (corruption check: string survived merge intact) ---
    if let LoroValue::Map(m) = components.get_deep_value() {
        for (eid, rec) in m.iter() {
            if let LoroValue::Map(comps) = rec {
                for (cname, cval) in comps.iter() {
                    if let LoroValue::Map(fields) = cval {
                        for (fname, fval) in fields.iter() {
                            if crate::scene::ASSET_FIELDS.contains(&fname.as_str()) {
                                let ok = vstr(fval).map(|s| {
                                    s.starts_with("assets/")
                                        && crate::scene::ASSET_EXTS.iter().any(|e| s.ends_with(&format!(".{e}")))
                                });
                                if ok != Some(true) {
                                    rep.add("corrupt-asset-ref", format!("{eid}.{cname}.{fname} = {fval:?}"));
                                }
                            }
                        }
                    } else {
                        rep.add("malformed-component", format!("{eid}.{cname}"));
                    }
                }
            } else {
                rep.add("malformed-component-record", eid.clone());
            }
        }
    }

    // --- bindings ---
    if let LoroValue::Map(m) = bindings.get_deep_value() {
        rep.edges = m.len();
        let mut seen_pairs: HashSet<(String, String, String)> = HashSet::new();
        for (key, val) in m.iter() {
            let LoroValue::Map(edge) = val else {
                rep.add("malformed-edge", key.clone());
                continue;
            };
            let from = edge.get("from").and_then(vstr);
            let to = edge.get("to").and_then(vstr);
            let kind = edge.get("kind").and_then(vstr);
            let (Some(from), Some(to), Some(kind)) = (from, to, kind) else {
                rep.add("edge-missing-fields", key.clone());
                continue;
            };
            if format!("{from}|{kind}|{to}") != *key {
                rep.add("edge-key-value-mismatch", format!("{key} vs {from}|{kind}|{to}"));
            }
            if !seen_pairs.insert((from.clone(), kind.clone(), to.clone())) {
                rep.add("duplicate-edge", key.clone());
            }
            if !eids_alive.contains(&from) {
                rep.add("dangling-edge-endpoint", format!("{key} (from {from} dead)"));
            }
            if !eids_alive.contains(&to) {
                rep.add("dangling-edge-endpoint", format!("{key} (to {to} dead)"));
            }
        }
    }

    rep
}

/// Deterministic deep-value serialization (sorted map keys) for convergence comparison.
pub fn canon(v: &LoroValue, out: &mut String) {
    match v {
        LoroValue::Null => out.push_str("null"),
        LoroValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        LoroValue::Double(d) => out.push_str(&format!("{d:?}")),
        LoroValue::I64(i) => out.push_str(&i.to_string()),
        LoroValue::Binary(b) => out.push_str(&format!("bin[{}]", b.len())),
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
        LoroValue::Container(c) => out.push_str(&format!("container({c})")),
    }
}

pub fn canon_doc(doc: &LoroDoc) -> String {
    let mut s = String::new();
    canon(&doc.get_deep_value(), &mut s);
    s
}
