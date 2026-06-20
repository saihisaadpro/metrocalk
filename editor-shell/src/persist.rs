//! Live persistence — **deterministic-seed + replay-log** (the scene survives close→reopen).
//!
//! On launch the shell rebuilds the scene by (1) re-seeding **deterministically** — same fixed seed →
//! byte-identical `EntityId`s, so a binding saved as `("1_5","1_a")` refers to the same entities next
//! launch — then (2) replaying an append-only log of the user's committed mutations on top.
//!
//! This deliberately avoids Loro export/`merge`-on-start: `merge` rebuilds the ECS from Loro but does
//! **not** restore the ECS capability pairs the reveal's `without(BindsTo,*)` exclusion needs (the
//! documented merge-drops-capabilities limitation — see `capscene::bind`). The edit log is the
//! `EditTx`/bind stream the editor already produces (the right shape), and replay goes back through
//! the **same commit pipeline** (invariant 3). After replay the caller calls
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
    /// Best-effort: a serialization or IO failure is dropped, never fatal — losing a persisted edit
    /// must not crash the editor.
    pub fn append(&self, rec: &Record) {
        let Ok(line) = serde_json::to_string(rec) else {
            return;
        };
        let is_empty = self.path.metadata().map_or(true, |m| m.len() == 0);
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            if is_empty {
                let _ = writeln!(f, "{HEADER_PREFIX}{}", self.fingerprint);
            }
            let _ = writeln!(f, "{line}");
        }
    }

    /// Replay the log onto `engine` (already deterministically seeded), each record back through the
    /// commit pipeline. Returns `(applied, skipped)`. **Fingerprint guard:** if the header is missing
    /// or names a different build, the log is from an incompatible id space — it is discarded (the
    /// file is cleared) and `(0, 0)` returned, rather than mis-binding saved ids. Otherwise a record
    /// that cannot apply — a malformed line, a rejected edit, or a bind referencing an id absent from
    /// the fresh seed (the **divergence** case) — is counted as skipped and never panics. The caller
    /// should `clear_history()` **after** replay so the restored scene is non-undoable.
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

    /// Delete the log (a "new scene" / reset). Best-effort.
    pub fn clear(&self) {
        let _ = std::fs::remove_file(&self.path);
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
