//! Live persistence — **deterministic-seed + replay-log** (the scene survives close→reopen).
//!
//! On launch the shell rebuilds the scene by (1) re-seeding **deterministically** — same fixed seed →
//! byte-identical `EntityId`s, so a binding saved as `("1_5","1_a")` refers to the same entities next
//! launch — then (2) replaying an append-only log of the user's committed mutations on top.
//!
//! This historically avoided Loro export/`merge`-on-start because `merge` rebuilt the ECS from Loro
//! but **not** the capability pairs the reveal's `without(BindsTo,*)` exclusion needs (the
//! merge-drops-capabilities limitation). That limitation is **now resolved** (ADR-032): a load/merge
//! re-derives caps from the durable document via the engine's `CapabilityResolver`, so a Loro-document
//! load is a viable load path — the M10.3 `.mtk` project format builds on it. This replay-log remains
//! the editor-**session** restore (deterministic seed + the `EditTx`/bind stream through the **same
//! commit pipeline**, invariant 3); the real-project save/open is the Loro document. After replay the
//! caller calls
//! [`Engine::clear_history`](metrocalk_core::Engine::clear_history) so the restored scene is
//! non-undoable (Ctrl-Z can't delete a restored world — the same guard as the seed).

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use metrocalk_core::marketplace::MarketplaceIndex;
use metrocalk_core::{Engine, EntityId};
use metrocalk_ecs::FlecsWorld;
use serde::{Deserialize, Serialize};

use crate::ai::{AiPatch, PatchOp};
use crate::bridge::{apply_edit, EditTx};
use crate::capscene::{self, CapScene, MeshCatalog, MESH_FIELD};

/// serde default for the `Record::Transform` `qw` / `scale` fields (an old log without them ⇒ identity
/// rotation / unit scale).
fn one() -> f64 {
    1.0
}

/// serde default for `Record::AiEdit::material` — an old log (pre-M11.2) had only the rustier edit.
fn rusty_material() -> String {
    crate::metering::RUSTY_MATERIAL_NAME.to_string()
}

/// The stdlib registry (components + events + actions) the M12.4 compose replay validates against — the same
/// vocabulary the live `author_rule`/`compose` paths use, rebuilt fresh for a (rare) replay of a `Compose`.
fn stdlib_registry() -> metrocalk_core::Registry<FlecsWorld> {
    let mut reg = metrocalk_core::Registry::new(FlecsWorld::new());
    for m in metrocalk_core::stdlib::standard_components() {
        let _ = reg.register(m);
    }
    for e in metrocalk_core::stdlib::standard_events() {
        reg.register_event(e);
    }
    for a in metrocalk_core::stdlib::standard_actions() {
        reg.register_action(a);
    }
    reg
}

/// One persisted user action, replayed in order to reconstruct the scene after a deterministic seed.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Record {
    /// A field edit (the `EditTx` the editor submitted).
    Edit(EditTx),
    /// A binding-by-intent (HealthBar → provider).
    Bind { from: String, to: String },
    /// A describe-to-create (M3.2): a free-text query resolved + instantiated at a position. Replayed
    /// deterministically (same resolve + same id allocation) so the described entity is recreated.
    /// With the asset tier (M4) the resolved kind may also carry a mesh handle — re-derived from the
    /// same catalog on replay, so a *visible* described object survives reload too.
    Describe { query: String, pos: [f32; 3] },
    /// A direct mesh placement (M4): an imported asset placed by **handle** at a position. Replayed by
    /// re-placing the same handle; the handle re-resolves against the reloaded (content-addressed)
    /// store, so the placed mesh survives close→reopen (ADR-013 id determinism).
    PlaceMesh { asset: String, pos: [f32; 3] },
    /// M11.3 (ADR-042) — an authored Light entity (kind = directional|point|spot, linear colour, intensity).
    /// Replayed deterministically (same id alloc) so the light survives close→reopen. Only the light ENTITY
    /// is persisted (Loro doc state); the per-frame lit result is a render projection, never logged.
    AddLight {
        // `light_kind` (not `kind`): the enum's serde tag is already `kind`.
        light_kind: String,
        pos: [f32; 3],
        color: [f32; 3],
        intensity: f32,
    },
    /// M11.4 (ADR-043) — an authored scene Camera entity (Transform pos + fov + active). Replayed by id
    /// (same alloc) so it survives close→reopen; the look-through view-proj is a render projection, never logged.
    AddCamera {
        pos: [f32; 3],
        fov: f32,
        active: bool,
    },
    /// M12.1 (ADR-045) — an authored rule (When/If/Then). The whole `RuleData` is kept so replay re-commits
    /// the same `SetRule` on the same id → the rule survives close→reopen (the Loro `rules` map is rebuilt
    /// from the replayed ops, same as every other doc state in the session-restore path).
    AuthorRule {
        id: String,
        rule: metrocalk_core::RuleData,
    },
    /// M12.1 (ADR-045) — a removed rule; replayed as the same `RemoveRule`.
    RemoveRule { id: String },
    /// M12.2 (ADR-046) — an authored state machine (states + transitions). The whole `StateMachine` is kept
    /// so replay re-commits the same `SetStateMachine` on the same id → the machine survives close→reopen
    /// (the Loro `state_machines` map is rebuilt from the replayed ops, exactly like the `rules` map).
    AuthorStateMachine {
        id: String,
        machine: metrocalk_core::StateMachine,
    },
    /// M12.2 (ADR-046) — a removed state machine; replayed as the same `RemoveStateMachine`.
    RemoveStateMachine { id: String },
    /// M12.3 (ADR-047) — a sandboxed WASM-plugin run: the plugin name + its JSON input. Replayed by
    /// **re-running** the (deterministic) plugin and re-applying its effect through the commit pipeline, so
    /// a plugin-driven scene change survives close→reopen. Replay relies on the plugin being deterministic
    /// (the registry's `PluginMeta.deterministic` gate) — same input → same effect.
    RunPlugin { name: String, input: String },
    /// M12.4 (ADR-048) — an applied AI **composition** (the validated op-set: SetField / AuthorRule /
    /// AuthorStateMachine). The whole `Composition` is kept so replay re-runs `apply_composition` on the same
    /// ids (the AI provides rule/machine ids; SetField targets existing entities) → the composed Rules /
    /// fields / machines survive close→reopen, rebuilt from the replayed ops exactly like a human edit.
    Compose {
        composition: metrocalk_core::compose::Composition,
    },
    /// A physics-body spawn (M8.2): a dynamic RigidBody + ball Collider + its ball mesh handle, at a
    /// position. Replayed by re-spawning deterministically (same id alloc); the sim body itself is
    /// RE-HYDRATED from the restored RigidBody entity by the engine thread after replay. Loro stores the
    /// SETUP intent, never the trajectory — ADR-021: sim-replay is a distinct channel from Loro
    /// time-travel (the sim regenerates the path from initial-state + ordered input + fixed dt).
    SpawnBody {
        pos: [f32; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mesh: Option<String>,
    },
    /// M8.3 make-dynamic: a dead mesh entity turned into a dynamic body (RigidBody + Collider added).
    /// Replayed by re-running it on the same id (deterministic); the engine thread re-hydrates the sim
    /// body from the restored components.
    MakeDynamic { id: String },
    /// M11.1 make-static: an existing (imported) mesh turned into a STATIC physics obstacle (a fixed
    /// RigidBody + a convex-hull Collider). Replayed by re-running it on the same id (deterministic).
    MakeStatic { id: String },
    /// M8.3 one-click physics fix (`add-collider`/`use-hull`/`fix-mass`/`fix-scale`) on an entity.
    /// Replayed by re-applying the same fix so the corrected setup survives reload.
    PhysicsFix { id: String, action: String },
    /// M8.5 interchange import (`format` = "urdf" | "usd"): the source text is kept so replay re-imports
    /// it through the same `Interchange` trait → identical registry components survive reload.
    Import { format: String, source: String },
    /// M9.1 transform-gizmo move: the entity's net world TRS — position (x/y/z), rotation quat
    /// (qx/qy/qz/qw), uniform scale — replayed as one `set_transform` commit so a moved/rotated/scaled
    /// entity reloads in its edited pose. The rotation/scale fields default (identity / 1.0) for old logs.
    Transform {
        id: String,
        x: f64,
        y: f64,
        z: f64,
        #[serde(default)]
        qx: f64,
        #[serde(default)]
        qy: f64,
        #[serde(default)]
        qz: f64,
        #[serde(default = "one")]
        qw: f64,
        #[serde(default = "one")]
        scale: f64,
    },
    /// M9.2 rigid part edit (G2): a part's net **LOCAL** TRS, stored as a sparse per-field **override**
    /// (ADR-026). Replayed by re-emitting the override (`set_part_local`) so the edited part reloads in
    /// its pose, overlaying the source/base by structure. The local is stored (not world) so replay is
    /// parent-independent + deterministic. Rotation/scale default to identity / 1.0 for old logs.
    EditPart {
        id: String,
        x: f64,
        y: f64,
        z: f64,
        #[serde(default)]
        qx: f64,
        #[serde(default)]
        qy: f64,
        #[serde(default)]
        qz: f64,
        #[serde(default = "one")]
        qw: f64,
        #[serde(default = "one")]
        scale: f64,
    },
    /// M9.2 reparent (G2 "drag in hierarchy"): move a part under a new parent (or to root when `parent`
    /// is `None`) — one `node.move` op. Replayed by id so the new hierarchy survives reload.
    Reparent {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
    },
    /// M9.2 deactivate-not-delete (G2): a part's `active` flag (USD deactivate ≡ reversible hide).
    /// Replayed by id so a removed part stays hidden (recoverable) after reload.
    SetPartActive { id: String, active: bool },
    /// M10.6 Delete = deactivate (R-NEXT-2): the toolbar/context Delete (deactivate + free dependents).
    /// Persisted so a "deleted" (recoverable) entity stays hidden across reload — replayed by id through
    /// the same `delete_deactivate`, so the projection re-emits `active:false` and the hierarchy dims it.
    DeleteDeactivate { id: String },
    /// M9.5 fidelity deformation (G5, ADR-029): a part's saved handle deform — the moved handle targets,
    /// stored as a G2 override (NOT baked geometry — the surface is reproduced deterministically from
    /// these). Replayed by re-emitting the override (`set_part_deform`) so the deformed surface survives
    /// close→reopen, exactly like a part transform.
    Deform {
        id: String,
        handles: Vec<(usize, [f32; 3])>,
    },
    /// A marketplace-tier apply (M5): a chosen pre-componentized entry, replayed deterministically by
    /// re-fetching it from the (checked-in) catalog by id + re-applying its namespaced caps + mesh
    /// handle, so a *marketplace*-sourced object survives reload exactly like a local one.
    ApplyMarketplace {
        entry_id: String,
        pos: [f32; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mesh: Option<String>,
    },
    /// A viewport **Remove** (M3.3): delete an entity + its edges (binding edges freed, dependents
    /// re-opened). Replayed by id so the removal survives reload.
    Remove { id: String },
    /// A viewport **Duplicate** (M3.3): clone an entity by source id. Replayed deterministically (same
    /// alloc sequence + fixed offset → the clone lands byte-identical), so it survives reload.
    Duplicate { source: String },
    /// A generation (M6): a grey placeholder + the streamed-in generated mesh **handle**. Replayed by
    /// re-placing the placeholder + re-applying the stored handle as a validated AI patch (the generated
    /// asset is content-addressed — for the deterministic fake it re-resolves; a novel real-provider
    /// asset persisting its bytes is a documented follow-up). `mesh = None` ⇒ generation hadn't completed.
    Generate {
        prompt: String,
        pos: [f32; 3],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mesh: Option<String>,
    },
    /// A live AI-edit (M7 + M11.2): the schema-validated material-assign patch on an entity (`material` =
    /// the named PBR preset, default `"rusty"` for an old log). Replayed by re-applying the patch (scene
    /// only — the wallet is a separate persisted ledger, so replay never re-charges), so the material
    /// survives close→reopen.
    AiEdit {
        id: String,
        #[serde(default = "rusty_material")]
        material: String,
    },
    /// An "+ Add" palette pick of a stdlib kind (M3.4) — replayed by re-instantiating the kind named
    /// `name` (the same path as `describe`), so a browsed-in object survives reload. (`name`, not `kind`:
    /// the enum's serde tag is already `kind`.)
    AddKind { name: String, pos: [f32; 3] },
    /// A single-step undo of the most recent action.
    Undo,
}

/// Header marking the build that wrote a log — its first line, `#mtk <fingerprint>`.
const HEADER_PREFIX: &str = "#mtk ";

/// An append-only edit log at `path` — a `#mtk <fingerprint>` header line then one JSON record per
/// line. The fingerprint ([`capscene::fingerprint`]) ties the log to the deterministic build that
/// wrote it; replay discards a log from an incompatible build rather than mis-binding saved ids.
pub struct Log {
    path: PathBuf,
    fingerprint: String,
}

impl Log {
    /// Open (lazily — the file is created on first append) a log at `path`, tied to `fingerprint`.
    #[must_use]
    pub fn open(path: PathBuf, fingerprint: String) -> Self {
        Self { path, fingerprint }
    }

    /// Append one record (one JSON line), writing the `#mtk` header first if the file is new/empty.
    /// Non-fatal (losing a persisted edit must not crash the editor) but NOT silent (audit F2): an IO
    /// failure here means the edit won't survive reload, so log it loudly rather than swallow it.
    pub fn append(&self, rec: &Record) {
        let Ok(line) = serde_json::to_string(rec) else {
            eprintln!("[persist] failed to serialize a record — edit not persisted (won't survive reload)");
            return;
        };
        let is_empty = self.path.metadata().map_or(true, |m| m.len() == 0);
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(mut f) => {
                if is_empty {
                    if let Err(e) = writeln!(f, "{HEADER_PREFIX}{}", self.fingerprint) {
                        eprintln!(
                            "[persist] failed to write log header to {}: {e}",
                            self.path.display()
                        );
                        return; // avoid a headerless log (replay would reject it on the fingerprint guard)
                    }
                }
                if let Err(e) = writeln!(f, "{line}") {
                    eprintln!(
                        "[persist] failed to append edit to {}: {e} (won't survive reload)",
                        self.path.display()
                    );
                }
            }
            Err(e) => eprintln!(
                "[persist] failed to open replay log {} — edit not persisted: {e}",
                self.path.display()
            ),
        }
    }

    /// Replay the log onto `engine` (already deterministically seeded), each record back through the
    /// commit pipeline. Returns `(applied, skipped)`. **Fingerprint guard:** if the header is missing
    /// or names a different build, the log is from an incompatible id space — it is discarded (the
    /// file is cleared) and `(0, 0)` returned, rather than mis-binding saved ids. Otherwise a record
    /// that cannot apply — a malformed line, a rejected edit, or a bind referencing an id absent from
    /// the fresh seed (the **divergence** case) — is counted as skipped and never panics. The caller
    /// should `clear_history()` **after** replay so the restored scene is non-undoable.
    #[allow(clippy::too_many_lines)] // one match arm per Record kind; splitting the dispatch hurts clarity
    pub fn replay(
        &self,
        engine: &mut Engine<FlecsWorld>,
        scene: &CapScene,
        catalog: &MeshCatalog,
    ) -> (usize, usize) {
        let Ok(file) = File::open(&self.path) else {
            return (0, 0); // no log yet → nothing to restore
        };
        let mut lines = BufReader::new(file).lines().map_while(Result::ok);
        let expected = format!("{HEADER_PREFIX}{}", self.fingerprint);
        match lines.next() {
            Some(h) if h == expected => {} // compatible build — replay below
            _ => {
                // missing/mismatched header → a log from an incompatible build. Discard it rather
                // than replay saved ids against a divergent scene (which would bind the wrong things).
                self.clear();
                return (0, 0);
            }
        }
        let (mut applied, mut skipped) = (0usize, 0usize);
        for line in lines {
            if line.trim().is_empty() || line.starts_with(HEADER_PREFIX) {
                continue;
            }
            let Ok(rec) = serde_json::from_str::<Record>(&line) else {
                skipped += 1;
                continue;
            };
            let ok = match rec {
                Record::Edit(tx) => apply_edit(engine, &tx).rejects.is_empty(),
                Record::Bind { from, to } => replay_bind(engine, scene, &from, &to),
                Record::Describe { query, pos } => {
                    capscene::describe_create(engine, scene, &query, pos, catalog).is_some()
                }
                Record::PlaceMesh { asset, pos } => {
                    capscene::place_mesh(engine, scene, &asset, pos).is_ok()
                }
                Record::AddLight {
                    light_kind,
                    pos,
                    color,
                    intensity,
                } => capscene::add_light(engine, scene, &light_kind, pos, color, intensity).is_ok(),
                Record::AddCamera { pos, fov, active } => {
                    capscene::add_camera(engine, scene, pos, fov, active).is_ok()
                }
                Record::AuthorRule { id, rule } => engine
                    .commit(
                        "author rule",
                        vec![metrocalk_core::Op::SetRule {
                            id: metrocalk_core::RuleId::new(id),
                            rule,
                        }],
                    )
                    .is_ok(),
                Record::RemoveRule { id } => engine
                    .commit(
                        "remove rule",
                        vec![metrocalk_core::Op::RemoveRule {
                            id: metrocalk_core::RuleId::new(id),
                        }],
                    )
                    .is_ok(),
                Record::AuthorStateMachine { id, machine } => engine
                    .commit(
                        "author state machine",
                        vec![metrocalk_core::Op::SetStateMachine {
                            id: metrocalk_core::StateMachineId::new(id),
                            sm: machine,
                        }],
                    )
                    .is_ok(),
                Record::RemoveStateMachine { id } => engine
                    .commit(
                        "remove state machine",
                        vec![metrocalk_core::Op::RemoveStateMachine {
                            id: metrocalk_core::StateMachineId::new(id),
                        }],
                    )
                    .is_ok(),
                Record::RunPlugin { name, input } => crate::plugin_host::run_plugin(
                    engine,
                    &metrocalk_core::stdlib::standard_components(),
                    &name,
                    &input,
                )
                .is_ok_and(|d| d.rejects.is_empty()),
                Record::Compose { composition } => {
                    // Re-run the same validated pipeline on the same ids — deterministic, so the composed
                    // Rules / fields / machines reload exactly (it validated when authored; its targets were
                    // created by earlier replayed records, so re-validation passes).
                    metrocalk_core::apply_composition(engine, &stdlib_registry(), &composition)
                        .is_ok()
                }
                Record::SpawnBody { pos, mesh } => {
                    capscene::spawn_physics_body(engine, scene, mesh.as_deref(), pos, 0.45).is_ok()
                }
                Record::MakeDynamic { id } => EntityId::from_loro_key(&id).is_some_and(|e| {
                    crate::physics_intent::make_dynamic(engine, scene, e, 1.0).is_ok()
                }),
                Record::MakeStatic { id } => EntityId::from_loro_key(&id)
                    .is_some_and(|e| crate::physics_intent::make_static(engine, scene, e).is_ok()),
                Record::PhysicsFix { id, action } => {
                    EntityId::from_loro_key(&id).is_some_and(|e| match action.as_str() {
                        "add-collider" => {
                            crate::physics_intent::add_collider(engine, scene, e, true).is_ok()
                        }
                        "use-hull" => crate::physics_intent::use_convex_hull(engine, e).is_ok(),
                        "fix-mass" => crate::physics_intent::fix_mass(engine, e, 1.0).is_ok(),
                        _ => true, // fix-scale: a flagged suggestion, no state change
                    })
                }
                Record::Import { format, source } => {
                    use metrocalk_interchange::{Interchange, UrdfInterchange, UsdInterchange};
                    let parsed = match format.as_str() {
                        "usd" => UsdInterchange.import(source.as_bytes()),
                        _ => UrdfInterchange.import(source.as_bytes()),
                    };
                    parsed
                        .ok()
                        .and_then(|imp| capscene::import_scene(engine, scene, &imp).ok())
                        .is_some()
                }
                #[allow(clippy::cast_possible_truncation)]
                Record::Transform {
                    id,
                    x,
                    y,
                    z,
                    qx,
                    qy,
                    qz,
                    qw,
                    scale,
                } => EntityId::from_loro_key(&id).is_some_and(|e| {
                    capscene::set_transform(
                        engine,
                        e,
                        [x as f32, y as f32, z as f32],
                        [qx as f32, qy as f32, qz as f32, qw as f32],
                        scale as f32,
                    )
                    .is_ok()
                }),
                #[allow(clippy::cast_possible_truncation)]
                Record::EditPart {
                    id,
                    x,
                    y,
                    z,
                    qx,
                    qy,
                    qz,
                    qw,
                    scale,
                } => EntityId::from_loro_key(&id).is_some_and(|e| {
                    capscene::set_part_local(
                        engine,
                        e,
                        [x as f32, y as f32, z as f32],
                        [qx as f32, qy as f32, qz as f32, qw as f32],
                        scale as f32,
                    )
                    .is_ok()
                }),
                Record::Reparent { id, parent } => EntityId::from_loro_key(&id).is_some_and(|e| {
                    let p = parent.as_deref().and_then(EntityId::from_loro_key);
                    capscene::reparent(engine, e, p).is_ok()
                }),
                Record::SetPartActive { id, active } => EntityId::from_loro_key(&id)
                    .is_some_and(|e| capscene::set_part_active(engine, e, active).is_ok()),
                Record::DeleteDeactivate { id } => EntityId::from_loro_key(&id)
                    .is_some_and(|e| capscene::delete_deactivate(engine, scene, e).is_ok()),
                Record::Deform { id, handles } => EntityId::from_loro_key(&id)
                    .is_some_and(|e| capscene::set_part_deform(engine, e, &handles).is_ok()),
                Record::ApplyMarketplace {
                    entry_id,
                    pos,
                    mesh,
                } => metrocalk_core::marketplace::LocalCatalog::builtin()
                    .get(&entry_id)
                    .is_some_and(|entry| {
                        capscene::apply_marketplace_entry(
                            engine,
                            scene,
                            &entry,
                            pos,
                            mesh.as_deref(),
                        )
                        .is_ok()
                    }),
                Record::Remove { id } => EntityId::from_loro_key(&id)
                    .is_some_and(|e| capscene::remove_entity(engine, scene, e).is_ok()),
                Record::Duplicate { source } => EntityId::from_loro_key(&source)
                    .is_some_and(|s| capscene::duplicate_entity(engine, scene, s).is_ok()),
                Record::Generate {
                    prompt: _,
                    pos,
                    mesh,
                } => replay_generate(engine, scene, pos, mesh),
                Record::AiEdit { id, material } => EntityId::from_loro_key(&id).is_some_and(|e| {
                    crate::ai::apply_ai_patch(
                        engine,
                        &metrocalk_core::stdlib::standard_components(),
                        "replay-ai-edit",
                        &crate::metering::material_patch(e, &material),
                    )
                    .rejects
                    .is_empty()
                }),
                Record::AddKind { name, pos } => {
                    capscene::add_kind(engine, scene, &name, pos, catalog).is_some()
                }
                Record::Undo => engine.undo(),
            };
            if ok {
                applied += 1;
            } else {
                skipped += 1;
            }
        }
        (applied, skipped)
    }

    /// Delete the log (a "new scene" / reset). Non-fatal, but a failure is logged (audit F9): a surviving
    /// stale log would replay old edits into a fresh scene (or re-trip the fingerprint guard every launch).
    /// `NotFound` is the normal "already clean" case and is not noise-worthy.
    pub fn clear(&self) {
        if let Err(e) = std::fs::remove_file(&self.path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "[persist] failed to clear replay log {}: {e}",
                    self.path.display()
                );
            }
        }
    }
}

fn replay_bind(engine: &mut Engine<FlecsWorld>, scene: &CapScene, from: &str, to: &str) -> bool {
    let (Some(f), Some(t)) = (EntityId::from_loro_key(from), EntityId::from_loro_key(to)) else {
        return false;
    };
    capscene::bind(engine, scene, f, t).is_ok()
}

/// Replay a generation: re-place the grey placeholder, then (if generation had completed) re-apply the
/// streamed-in mesh **handle** as a validated AI patch — exactly the live path, so the generated object
/// is reconstructed in its final state. The placeholder lands at the same deterministic id.
fn replay_generate(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    pos: [f32; 3],
    mesh: Option<String>,
) -> bool {
    let Ok(id) = capscene::place_generation_placeholder(engine, scene, pos) else {
        return false;
    };
    let Some(handle) = mesh else {
        return true; // placeholder-only (generation hadn't completed before the export)
    };
    let patch = AiPatch {
        client_op_id: "replay-generate".to_string(),
        ops: vec![PatchOp::SetField {
            id: id.to_loro_key(),
            component: "MeshRenderer".to_string(),
            field: MESH_FIELD.to_string(),
            value: serde_json::Value::String(handle),
        }],
    };
    crate::ai::apply_ai_patch(
        engine,
        &metrocalk_core::stdlib::standard_components(),
        "replay-generate-swap",
        &patch,
    )
    .rejects
    .is_empty()
}
