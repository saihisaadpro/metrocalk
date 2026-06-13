//! Synthetic Metrocalk scene modeled as a Loro document, plus the mutation engine.
//!
//! Document layout:
//!   tree "hierarchy"   — entity hierarchy (MovableTree); node meta holds the stable eid
//!   map  "components"  — eid -> map(componentName -> map(field -> value))
//!   map  "bindings"    — "from|kind|to" -> map{from, to, kind}
//!
//! A spike-side shadow model (entity list, edge list) is kept so mutation *preparation*
//! (choosing targets, validating reparents) stays out of the timed sections.

use crate::rng::Rng;
use loro::{Container, LoroDoc, LoroMap, LoroTree, LoroValue, TreeID, TreeParentId, ValueOrContainer};

/// Idempotent get-or-create of a child Map under `parent[key]`, using a *regular* CRDT container.
///
/// We deliberately avoid Loro's `ensure_mergeable_map` helper: it does not round-trip through the
/// `UndoManager` (undo+redo turns the mergeable container into a regular one, after which the
/// helper errors — see README finding F1). Regular containers mean concurrent first-writes to the
/// same key fork into two containers; reconciling that is the job of the merge-validation layer
/// (ADR-002), which is exactly where the saved WAL effort is meant to go.
pub fn child_map(parent: &LoroMap, key: &str) -> LoroMap {
    match parent.get(key) {
        Some(ValueOrContainer::Container(Container::Map(m))) => m,
        _ => parent.insert_container(key, LoroMap::new()).unwrap(),
    }
}

#[derive(Clone, Copy)]
pub enum FieldKind {
    F64,
    I64,
    Bool,
    Str,
    Asset,
}
use FieldKind::*;

pub const COMPONENTS: &[(&str, &[(&str, FieldKind)])] = &[
    ("Transform", &[("px", F64), ("py", F64), ("pz", F64), ("rx", F64), ("ry", F64), ("rz", F64), ("sx", F64), ("sy", F64), ("sz", F64)]),
    ("MeshRenderer", &[("mesh", Asset), ("material", Asset), ("castShadows", Bool)]),
    ("Sprite", &[("texture", Asset), ("layer", I64), ("flipX", Bool)]),
    ("Health", &[("hp", I64), ("maxHp", I64), ("regen", F64)]),
    ("Script", &[("source", Asset), ("enabled", Bool)]),
    ("Collider", &[("shape", Str), ("isTrigger", Bool), ("friction", F64)]),
    ("Rigidbody", &[("mass", F64), ("kinematic", Bool), ("drag", F64)]),
    ("AudioSource", &[("clip", Asset), ("volume", F64), ("looping", Bool)]),
    ("Animator", &[("controller", Asset), ("speed", F64)]),
    ("Light", &[("intensity", F64), ("color", Str), ("range", F64)]),
];

pub const ASSET_FIELDS: &[&str] = &["mesh", "material", "texture", "source", "clip", "controller"];
pub const ASSET_EXTS: &[&str] = &["glb", "png", "wav", "wgsl"];
pub const EDGE_KINDS: &[&str] = &["bindsTo", "observes", "drives", "follows"];
const STR_POOL: &[&str] = &["box", "sphere", "capsule", "#ffffff", "#ff0000", "idle", "run"];

pub fn field_value(kind: FieldKind, rng: &mut Rng) -> LoroValue {
    match kind {
        F64 => LoroValue::from(rng.f64() * 100.0),
        I64 => LoroValue::from(rng.below(1000) as i64),
        Bool => LoroValue::from(rng.next() & 1 == 0),
        Str => LoroValue::from(STR_POOL[rng.below(STR_POOL.len())]),
        Asset => LoroValue::from(format!(
            "assets/asset_{:04x}.{}",
            rng.below(0x10000),
            ASSET_EXTS[rng.below(ASSET_EXTS.len())]
        )),
    }
}

pub struct Entity {
    pub eid: String,
    pub tid: TreeID,
    /// indices into COMPONENTS
    pub comps: Vec<usize>,
}

pub struct Edge {
    pub key: String,
    pub from: String,
    pub to: String,
}

pub struct Scene {
    pub doc: LoroDoc,
    pub tree: LoroTree,
    pub components: LoroMap,
    pub bindings: LoroMap,
    pub entities: Vec<Entity>,
    pub edges: Vec<Edge>,
    pub next_eid: u64,
}

impl Scene {
    pub fn generate(seed: u64, n_entities: usize, n_edges: usize, peer: u64) -> Scene {
        let mut rng = Rng::new(seed);
        let doc = LoroDoc::new();
        doc.set_peer_id(peer).unwrap();
        let mut s = Scene {
            tree: doc.get_tree("hierarchy"),
            components: doc.get_map("components"),
            bindings: doc.get_map("bindings"),
            doc,
            entities: Vec::with_capacity(n_entities),
            edges: Vec::with_capacity(n_edges),
            next_eid: 0,
        };
        for i in 0..n_entities {
            let parent: TreeParentId = if i == 0 {
                TreeParentId::Root
            } else {
                TreeParentId::Node(s.entities[rng.below(s.entities.len())].tid)
            };
            let comps = random_comp_set(&mut rng);
            let eid = s.fresh_eid();
            s.create_entity(&eid, parent, &comps, &mut rng);
            if i % 256 == 255 {
                s.doc.commit();
            }
        }
        s.doc.commit();
        let mut made = 0;
        while made < n_edges {
            if let Some(p) = s.prepare_bind_add(&mut rng) {
                s.exec_bind_add(&p);
                s.finish_bind_add(p);
                made += 1;
            }
        }
        s.doc.commit();
        s
    }

    /// Bind this shadow model to a forked copy of the doc (TreeIDs are identical across forks).
    pub fn rebind(&self, doc: LoroDoc) -> Scene {
        Scene {
            tree: doc.get_tree("hierarchy"),
            components: doc.get_map("components"),
            bindings: doc.get_map("bindings"),
            doc,
            entities: self
                .entities
                .iter()
                .map(|e| Entity { eid: e.eid.clone(), tid: e.tid, comps: e.comps.clone() })
                .collect(),
            edges: self
                .edges
                .iter()
                .map(|e| Edge { key: e.key.clone(), from: e.from.clone(), to: e.to.clone() })
                .collect(),
            next_eid: self.next_eid,
        }
    }

    fn fresh_eid(&mut self) -> String {
        let eid = format!("e{:06}", self.next_eid);
        self.next_eid += 1;
        eid
    }

    fn create_entity(&mut self, eid: &str, parent: TreeParentId, comps: &[usize], rng: &mut Rng) -> TreeID {
        let tid = self.tree.create(parent).unwrap();
        self.tree.get_meta(tid).unwrap().insert("eid", eid).unwrap();
        let rec = child_map(&self.components, eid);
        for &ci in comps {
            let (name, fields) = COMPONENTS[ci];
            let cmap = child_map(&rec, name);
            for &(fname, kind) in fields {
                cmap.insert(fname, field_value(kind, rng)).unwrap();
            }
        }
        self.entities.push(Entity { eid: eid.to_string(), tid, comps: comps.to_vec() });
        tid
    }

    // ---------------- mutation engine ----------------

    pub fn prepare(&self, rng: &mut Rng) -> Prepared {
        let roll = rng.below(100);
        // Each arm degrades to a guaranteed-available prop-set if its preferred op can't be
        // formed (e.g. no fresh edge pair found) so the mix stays close to target without panics.
        if roll < 70 {
            self.prepare_prop_set(rng)
        } else if roll < 80 {
            self.prepare_reparent(rng).unwrap_or_else(|| self.prepare_prop_set(rng))
        } else if roll < 85 {
            self.prepare_create(rng)
        } else if roll < 90 {
            self.prepare_delete(rng).unwrap_or_else(|| self.prepare_prop_set(rng))
        } else if roll < 95 {
            self.prepare_bind_add(rng).map(Prepared::BindAdd).unwrap_or_else(|| self.prepare_prop_set(rng))
        } else {
            self.prepare_bind_remove(rng)
                .or_else(|| self.prepare_bind_add(rng).map(Prepared::BindAdd))
                .unwrap_or_else(|| self.prepare_prop_set(rng))
        }
    }

    fn prepare_prop_set(&self, rng: &mut Rng) -> Prepared {
        let e = &self.entities[rng.below(self.entities.len())];
        let ci = e.comps[rng.below(e.comps.len())];
        let (name, fields) = COMPONENTS[ci];
        let (fname, kind) = fields[rng.below(fields.len())];
        Prepared::PropSet {
            eid: e.eid.clone(),
            comp: name,
            field: fname,
            value: field_value(kind, rng),
        }
    }

    fn prepare_reparent(&self, rng: &mut Rng) -> Option<Prepared> {
        for _ in 0..20 {
            let child = &self.entities[rng.below(self.entities.len())];
            let parent = &self.entities[rng.below(self.entities.len())];
            if child.tid == parent.tid {
                continue;
            }
            // reject if `parent` is inside `child`'s subtree (would create a cycle)
            let mut cur = parent.tid;
            let mut ok = true;
            loop {
                match self.tree.parent(cur) {
                    Some(TreeParentId::Node(p)) => {
                        if p == child.tid {
                            ok = false;
                            break;
                        }
                        cur = p;
                    }
                    _ => break,
                }
            }
            if ok {
                return Some(Prepared::Reparent { tid: child.tid, new_parent: parent.tid });
            }
        }
        None
    }

    fn prepare_create(&self, rng: &mut Rng) -> Prepared {
        let parent = self.entities[rng.below(self.entities.len())].tid;
        let comps = random_comp_set(rng);
        // pre-generate all field values so execute() is pure mutation
        let mut payload = Vec::new();
        for &ci in &comps {
            let (name, fields) = COMPONENTS[ci];
            let vals: Vec<(&'static str, LoroValue)> =
                fields.iter().map(|&(f, k)| (f, field_value(k, rng))).collect();
            payload.push((name, vals));
        }
        Prepared::Create { parent, comps, payload, eid: String::new() }
    }

    fn prepare_delete(&self, rng: &mut Rng) -> Option<Prepared> {
        'outer: for _ in 0..20 {
            let idx = rng.below(self.entities.len());
            if idx == 0 {
                continue; // keep the scene root
            }
            let root = &self.entities[idx];
            // collect the subtree; reject large ones (realistic deletes are small)
            let mut tids = vec![root.tid];
            let mut i = 0;
            while i < tids.len() {
                if let Some(children) = self.tree.children(TreeParentId::Node(tids[i])) {
                    tids.extend(children);
                }
                if tids.len() > 25 {
                    continue 'outer;
                }
                i += 1;
            }
            let tidset: std::collections::HashSet<TreeID> = tids.iter().copied().collect();
            let eids: Vec<String> = self
                .entities
                .iter()
                .filter(|e| tidset.contains(&e.tid))
                .map(|e| e.eid.clone())
                .collect();
            let eidset: std::collections::HashSet<&str> = eids.iter().map(|s| s.as_str()).collect();
            let edge_keys: Vec<String> = self
                .edges
                .iter()
                .filter(|e| eidset.contains(e.from.as_str()) || eidset.contains(e.to.as_str()))
                .map(|e| e.key.clone())
                .collect();
            return Some(Prepared::Delete { tid: root.tid, tids, eids, edge_keys });
        }
        None
    }

    fn prepare_bind_add(&self, rng: &mut Rng) -> Option<PreparedBind> {
        for _ in 0..20 {
            let from = &self.entities[rng.below(self.entities.len())];
            let to = &self.entities[rng.below(self.entities.len())];
            if from.tid == to.tid {
                continue;
            }
            let kind = EDGE_KINDS[rng.below(EDGE_KINDS.len())];
            let key = format!("{}|{}|{}", from.eid, kind, to.eid);
            if self.edges.iter().any(|e| e.key == key) {
                continue;
            }
            return Some(PreparedBind { key, from: from.eid.clone(), to: to.eid.clone(), kind });
        }
        None
    }

    fn prepare_bind_remove(&self, rng: &mut Rng) -> Option<Prepared> {
        if self.edges.is_empty() {
            return None;
        }
        Some(Prepared::BindRemove { key: self.edges[rng.below(self.edges.len())].key.clone() })
    }

    /// The timed part: Loro mutations + one commit. No allocation-heavy prep, no shadow updates.
    /// Returns the new TreeID for a Create so the shadow model records the real id (no scan).
    pub fn execute(&self, p: &Prepared) -> Option<TreeID> {
        let mut created = None;
        match p {
            Prepared::PropSet { eid, comp, field, value } => {
                let rec = child_map(&self.components, eid);
                let cmap = child_map(&rec, comp);
                cmap.insert(field, value.clone()).unwrap();
            }
            Prepared::Reparent { tid, new_parent } => {
                self.tree.mov(*tid, *new_parent).unwrap();
            }
            Prepared::Create { parent, payload, eid, .. } => {
                let tid = self.tree.create(TreeParentId::Node(*parent)).unwrap();
                self.tree.get_meta(tid).unwrap().insert("eid", eid.as_str()).unwrap();
                let rec = child_map(&self.components, eid);
                for (name, vals) in payload {
                    let cmap = child_map(&rec, name);
                    for (f, v) in vals {
                        cmap.insert(f, v.clone()).unwrap();
                    }
                }
                created = Some(tid);
            }
            Prepared::Delete { tid, eids, edge_keys, .. } => {
                self.tree.delete(*tid).unwrap();
                for eid in eids {
                    self.components.delete(eid).unwrap();
                }
                for k in edge_keys {
                    self.bindings.delete(k).unwrap();
                }
            }
            Prepared::BindAdd(b) => self.exec_bind_add(b),
            Prepared::BindRemove { key } => {
                self.bindings.delete(key).unwrap();
            }
        }
        self.doc.commit();
        created
    }

    fn exec_bind_add(&self, b: &PreparedBind) {
        let em = child_map(&self.bindings, &b.key);
        em.insert("from", b.from.as_str()).unwrap();
        em.insert("to", b.to.as_str()).unwrap();
        em.insert("kind", b.kind).unwrap();
    }

    /// Shadow-model bookkeeping, untimed.
    pub fn finish(&mut self, p: Prepared) {
        match p {
            Prepared::PropSet { .. } => {}
            Prepared::Reparent { .. } => {}
            Prepared::Create { .. } => {} // shadow update handled in run_mutation (needs the new TreeID)
            Prepared::Delete { tids, eids, edge_keys, .. } => {
                let tidset: std::collections::HashSet<TreeID> = tids.into_iter().collect();
                self.entities.retain(|e| !tidset.contains(&e.tid));
                let _ = eids;
                let keyset: std::collections::HashSet<String> = edge_keys.into_iter().collect();
                self.edges.retain(|e| !keyset.contains(&e.key));
            }
            Prepared::BindAdd(b) => self.finish_bind_add(b),
            Prepared::BindRemove { key } => {
                self.edges.retain(|e| e.key != key);
            }
        }
    }

    fn finish_bind_add(&mut self, b: PreparedBind) {
        self.edges.push(Edge { key: b.key, from: b.from, to: b.to });
    }

    /// Run one full random mutation (prepare → execute → finish), returning the timed duration.
    pub fn run_mutation(&mut self, rng: &mut Rng) -> std::time::Duration {
        let mut p = self.prepare(rng);
        if let Prepared::Create { eid, .. } = &mut p {
            *eid = self.fresh_eid();
        }
        let t = std::time::Instant::now();
        let created = self.execute(&p);
        let dt = t.elapsed();
        // Record the new entity in the shadow with the real TreeID returned by tree.create.
        if let (Prepared::Create { eid, comps, .. }, Some(tid)) = (&p, created) {
            self.entities.push(Entity { eid: eid.clone(), tid, comps: comps.clone() });
        }
        self.finish(p);
        dt
    }
}

fn random_comp_set(rng: &mut Rng) -> Vec<usize> {
    // Transform always present + 2..=7 distinct others = 3..=8 total
    let extra = rng.range(2, 7);
    let mut comps = vec![0usize];
    while comps.len() < extra + 1 {
        let c = rng.range(1, COMPONENTS.len() - 1);
        if !comps.contains(&c) {
            comps.push(c);
        }
    }
    comps
}

pub enum Prepared {
    PropSet { eid: String, comp: &'static str, field: &'static str, value: LoroValue },
    Reparent { tid: TreeID, new_parent: TreeID },
    Create { parent: TreeID, eid: String, comps: Vec<usize>, payload: Vec<(&'static str, Vec<(&'static str, LoroValue)>)> },
    Delete { tid: TreeID, tids: Vec<TreeID>, eids: Vec<String>, edge_keys: Vec<String> },
    BindAdd(PreparedBind),
    BindRemove { key: String },
}

pub struct PreparedBind {
    pub key: String,
    pub from: String,
    pub to: String,
    pub kind: &'static str,
}
