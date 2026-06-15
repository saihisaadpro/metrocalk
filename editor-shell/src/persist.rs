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

use metrocalk_core::{Engine, EntityId};
use metrocalk_ecs::FlecsWorld;
use serde::{Deserialize, Serialize};

use crate::bridge::{apply_edit, EditTx};
use crate::capscene::{self, CapScene};

/// One persisted user action, replayed in order to reconstruct the scene after a deterministic seed.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Record {
    /// A field edit (the `EditTx` the editor submitted).
    Edit(EditTx),
    /// A binding-by-intent (HealthBar → provider).
    Bind { from: String, to: String },
    /// A single-step undo of the most recent action.
    Undo,
}

/// An append-only edit log at `path` — one JSON record per line.
pub struct Log {
    path: PathBuf,
}

impl Log {
    /// Open (lazily — the file is created on first append) a log at `path`.
    #[must_use]
    pub fn open(path: PathBuf) -> Self {
        Self { path }
    }

    /// Append one record (one JSON line). Best-effort: a serialization or IO failure is dropped, never
    /// fatal — losing a persisted edit must not crash the editor.
    pub fn append(&self, rec: &Record) {
        let Ok(line) = serde_json::to_string(rec) else {
            return;
        };
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Replay the log onto `engine` (already deterministically seeded), each record back through the
    /// commit pipeline. Returns `(applied, skipped)`. A record that cannot apply — a malformed line, a
    /// rejected edit, or a bind referencing an id absent from the fresh seed (the **divergence** case)
    /// — is counted as skipped and never panics. The caller should `clear_history()` **after** replay
    /// so the restored scene is non-undoable.
    pub fn replay(&self, engine: &mut Engine<FlecsWorld>, scene: &CapScene) -> (usize, usize) {
        let Ok(file) = File::open(&self.path) else {
            return (0, 0); // no log yet → nothing to restore
        };
        let (mut applied, mut skipped) = (0usize, 0usize);
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(rec) = serde_json::from_str::<Record>(&line) else {
                skipped += 1;
                continue;
            };
            let ok = match rec {
                Record::Edit(tx) => apply_edit(engine, &tx).rejects.is_empty(),
                Record::Bind { from, to } => replay_bind(engine, scene, &from, &to),
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
