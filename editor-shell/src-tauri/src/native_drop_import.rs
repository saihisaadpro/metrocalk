//! Native OS file-drop intake.
//!
//! The window callback is part of tao/WebView2's OLE modal drag loop on Windows. It must only classify
//! and enqueue: waiting for the serial engine there prevents `DoDragDrop` from returning and makes the
//! whole window appear hung. One bounded worker preserves user drop order, applies backpressure without
//! blocking that callback, and still routes every accepted file through `EngineCmd::ImportAsset`.

use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::Emitter;

use super::{EngineCmd, ImportAssetResult};

pub(crate) const IMPORT_LIFECYCLE_EVENT: &str = "mtk://import-lifecycle";
const DROP_QUEUE_CAPACITY: usize = 8;
const REPLY_POLL: Duration = Duration::from_secs(2);
const DELAYED_AFTER: Duration = Duration::from_secs(15);
const DELAYED_REPEAT: Duration = Duration::from_secs(30);

static NEXT_BATCH_ID: AtomicU64 = AtomicU64::new(1);

const CANCELLATION_OPEN: u8 = 0;
const CANCELLATION_REQUESTED: u8 = 1;
const CANCELLATION_SEALED: u8 = 2;

/// One batch-owned cancellation signal shared by the Tauri command, drop worker and serial engine.
///
/// `requested` is deliberately an `Arc<AtomicBool>`: the UI command can signal it without waiting for
/// the engine thread that may currently be parsing a large CAD container. `phase` closes the tiny race at
/// the final commit boundary: either cancellation wins while the token is open, or the import seals the
/// token and the command truthfully reports that the commit checkpoint has already passed.
#[derive(Clone, Debug)]
pub(crate) struct ImportCancellation {
    requested: Arc<AtomicBool>,
    phase: Arc<AtomicU8>,
}

impl ImportCancellation {
    pub(crate) fn new() -> Self {
        Self {
            requested: Arc::new(AtomicBool::new(false)),
            phase: Arc::new(AtomicU8::new(CANCELLATION_OPEN)),
        }
    }

    pub(crate) fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
            || self.phase.load(Ordering::Acquire) == CANCELLATION_REQUESTED
    }

    pub(crate) fn request(&self) -> bool {
        if self
            .phase
            .compare_exchange(
                CANCELLATION_OPEN,
                CANCELLATION_REQUESTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.requested.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Atomically close the cancellation window immediately before a scene mutation.
    /// `false` means a cancellation request won and the caller must not commit.
    pub(crate) fn seal_for_commit(&self) -> bool {
        match self.phase.compare_exchange(
            CANCELLATION_OPEN,
            CANCELLATION_SEALED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) | Err(CANCELLATION_SEALED) => true,
            Err(CANCELLATION_REQUESTED) => false,
            Err(_) => false,
        }
    }
}

/// Direct, non-engine-thread control plane for native drop batches.
#[derive(Clone, Default)]
pub(crate) struct NativeImportControl {
    active: Arc<Mutex<HashMap<u64, ImportCancellation>>>,
}

impl NativeImportControl {
    fn begin(&self, batch_id: u64) -> ImportCancellation {
        let cancellation = ImportCancellation::new();
        self.active
            .lock()
            .unwrap()
            .insert(batch_id, cancellation.clone());
        cancellation
    }

    pub(crate) fn cancel(&self, batch_id: u64) -> bool {
        let cancellation = self.active.lock().unwrap().get(&batch_id).cloned();
        cancellation.is_some_and(|cancellation| cancellation.request())
    }

    fn finish(&self, batch_id: u64) {
        self.active.lock().unwrap().remove(&batch_id);
    }

    #[cfg(test)]
    fn contains(&self, batch_id: u64) -> bool {
        self.active.lock().unwrap().contains_key(&batch_id)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DropFile {
    pub(crate) name: String,
    pub(crate) supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ImportProgressStage {
    Queued,
    Importing,
    Delayed,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub(crate) enum ImportedSubject {
    Entity { root_id: String },
    Environment { label: String },
}

/// Stable, structured UI contract for the complete native-drop lifecycle.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub(crate) enum ImportLifecycleEvent {
    Hovered {
        files: Vec<DropFile>,
    },
    Left,
    Dropped {
        batch_id: u64,
        files: Vec<DropFile>,
    },
    Progress {
        batch_id: u64,
        index: usize,
        total: usize,
        completed: usize,
        file_name: String,
        stage: ImportProgressStage,
        elapsed_ms: u64,
    },
    Succeeded {
        batch_id: u64,
        index: usize,
        total: usize,
        file_name: String,
        subject: ImportedSubject,
        message: String,
    },
    Refused {
        batch_id: Option<u64>,
        index: usize,
        total: usize,
        file_name: String,
        message: String,
        recoverable: bool,
    },
    Failed {
        batch_id: u64,
        index: usize,
        total: usize,
        file_name: String,
        message: String,
        recoverable: bool,
    },
    Cancelled {
        batch_id: u64,
        index: usize,
        total: usize,
        file_name: String,
        message: String,
    },
}

#[derive(Clone, Debug)]
struct DropCandidate {
    path: PathBuf,
    file: DropFile,
}

#[derive(Debug)]
struct DropBatch {
    id: u64,
    candidates: Vec<DropCandidate>,
    cancellation: ImportCancellation,
}

type LifecycleEmitter = Arc<dyn Fn(ImportLifecycleEvent) + Send + Sync + 'static>;

#[derive(Clone, Copy)]
struct WorkerTiming {
    reply_poll: Duration,
    delayed_after: Duration,
    delayed_repeat: Duration,
}

impl Default for WorkerTiming {
    fn default() -> Self {
        Self {
            reply_poll: REPLY_POLL,
            delayed_after: DELAYED_AFTER,
            delayed_repeat: DELAYED_REPEAT,
        }
    }
}

/// Install the native listener and its one bounded serial worker.
pub(crate) fn install<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    app: tauri::AppHandle<R>,
    engine_tx: mpsc::Sender<EngineCmd>,
    control: NativeImportControl,
) {
    let event_app = app.clone();
    let emitter: LifecycleEmitter = Arc::new(move |event| {
        if let Err(error) = event_app.emit(IMPORT_LIFECYCLE_EVENT, event) {
            eprintln!("[shell] native import lifecycle event could not reach the UI: {error}");
        }
    });

    let (batch_tx, batch_rx) = mpsc::sync_channel(DROP_QUEUE_CAPACITY);
    let worker_emitter = Arc::clone(&emitter);
    let worker_control = control.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("native-import-drop-worker".into())
        .spawn(move || {
            run_worker(
                batch_rx,
                engine_tx,
                worker_emitter,
                worker_control,
                WorkerTiming::default(),
            );
        })
    {
        // `batch_rx` was dropped with the failed spawn closure, so future drops truthfully report that the
        // service is unavailable rather than accumulating work with nobody able to consume it.
        eprintln!("[shell] native import worker could not start: {error}");
    }

    let callback_emitter = Arc::clone(&emitter);
    let callback_control = control;
    window.on_window_event(move |event| {
        let tauri::WindowEvent::DragDrop(drag) = event else {
            return;
        };
        match drag {
            tauri::DragDropEvent::Enter { paths, .. } => {
                callback_emitter(ImportLifecycleEvent::Hovered {
                    files: classify_paths(paths)
                        .into_iter()
                        .map(|candidate| candidate.file)
                        .collect(),
                });
            }
            // `Over` can fire at pointer-message frequency; it carries no new file information and emitting
            // it would turn a user gesture into an avoidable UI/event flood.
            tauri::DragDropEvent::Over { .. } => {}
            tauri::DragDropEvent::Leave => callback_emitter(ImportLifecycleEvent::Left),
            tauri::DragDropEvent::Drop { paths, .. } => {
                dispatch_drop(paths, &batch_tx, &callback_emitter, &callback_control);
            }
            _ => {}
        }
    });
}

fn classify_paths(paths: &[PathBuf]) -> Vec<DropCandidate> {
    paths.iter().map(|path| classify_path(path)).collect()
}

fn classify_path(path: &Path) -> DropCandidate {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let supported = metrocalk_editor_shell::spec_for_extension(&extension)
        .is_some_and(|format| format.available && format.direction.reads());
    let reason = (!supported).then(|| metrocalk_editor_shell::explain_unsupported(&name));
    DropCandidate {
        path: path.to_path_buf(),
        file: DropFile {
            name,
            supported,
            reason,
        },
    }
}

/// Enqueue a whole OS drop without ever waiting for capacity. `try_send` is the key OLE-loop invariant.
fn dispatch_drop(
    paths: &[PathBuf],
    batch_tx: &SyncSender<DropBatch>,
    emitter: &LifecycleEmitter,
    control: &NativeImportControl,
) {
    if paths.is_empty() {
        return;
    }
    let id = NEXT_BATCH_ID.fetch_add(1, Ordering::Relaxed);
    let batch = DropBatch {
        id,
        candidates: classify_paths(paths),
        cancellation: control.begin(id),
    };
    emitter(ImportLifecycleEvent::Dropped {
        batch_id: batch.id,
        files: batch
            .candidates
            .iter()
            .map(|candidate| candidate.file.clone())
            .collect(),
    });
    match batch_tx.try_send(batch) {
        Ok(()) => {}
        Err(TrySendError::Full(batch)) => {
            control.finish(batch.id);
            reject_unqueued(
                batch,
                "The import queue is full. Wait for the current imports to finish, then drop this file again.",
                emitter,
            );
        }
        Err(TrySendError::Disconnected(batch)) => {
            control.finish(batch.id);
            reject_unqueued(
                batch,
                "The import service is unavailable. Restart the editor, then try this drop again.",
                emitter,
            );
        }
    }
}

fn reject_unqueued(batch: DropBatch, unavailable: &str, emitter: &LifecycleEmitter) {
    let total = batch.candidates.len();
    for (offset, candidate) in batch.candidates.into_iter().enumerate() {
        let index = offset + 1;
        if candidate.file.supported {
            emitter(ImportLifecycleEvent::Failed {
                batch_id: batch.id,
                index,
                total,
                file_name: candidate.file.name,
                message: unavailable.to_string(),
                recoverable: true,
            });
        } else {
            emitter(ImportLifecycleEvent::Refused {
                batch_id: Some(batch.id),
                index,
                total,
                file_name: candidate.file.name,
                message: candidate
                    .file
                    .reason
                    .unwrap_or_else(|| "This file type is not available in this build.".into()),
                recoverable: false,
            });
        }
    }
}

fn run_worker(
    batches: Receiver<DropBatch>,
    engine_tx: mpsc::Sender<EngineCmd>,
    emitter: LifecycleEmitter,
    control: NativeImportControl,
    timing: WorkerTiming,
) {
    while let Ok(batch) = batches.recv() {
        let total = batch.candidates.len();
        for (offset, candidate) in batch.candidates.into_iter().enumerate() {
            let index = offset + 1;
            let completed = offset;
            if batch.cancellation.is_requested() {
                emit_cancelled(
                    &emitter,
                    batch.id,
                    index,
                    total,
                    &candidate.file.name,
                );
                continue;
            }
            if !candidate.file.supported {
                emitter(ImportLifecycleEvent::Refused {
                    batch_id: Some(batch.id),
                    index,
                    total,
                    file_name: candidate.file.name,
                    message: candidate
                        .file
                        .reason
                        .unwrap_or_else(|| "This file type is not available in this build.".into()),
                    recoverable: false,
                });
                continue;
            }

            emit_progress(
                &emitter,
                batch.id,
                index,
                total,
                completed,
                &candidate.file.name,
                ImportProgressStage::Queued,
                Duration::ZERO,
            );
            let (reply, reply_rx) = mpsc::channel();
            if engine_tx
                .send(EngineCmd::ImportAsset {
                    path: candidate.path.display().to_string(),
                    cancellation: Some(batch.cancellation.clone()),
                    reply,
                })
                .is_err()
            {
                emitter(ImportLifecycleEvent::Failed {
                    batch_id: batch.id,
                    index,
                    total,
                    file_name: candidate.file.name,
                    message: "The engine is unavailable. Restart the editor, then try this import again."
                        .into(),
                    recoverable: true,
                });
                continue;
            }

            let started = Instant::now();
            emit_progress(
                &emitter,
                batch.id,
                index,
                total,
                completed,
                &candidate.file.name,
                ImportProgressStage::Importing,
                Duration::ZERO,
            );
            let mut next_delayed = timing.delayed_after;
            loop {
                match reply_rx.recv_timeout(timing.reply_poll) {
                    Ok(ImportAssetResult::Imported(created)) => {
                        let subject = match created.strip_prefix("env:") {
                            Some(label) => ImportedSubject::Environment {
                                label: label.to_string(),
                            },
                            None => ImportedSubject::Entity { root_id: created },
                        };
                        let message = match &subject {
                            ImportedSubject::Entity { .. } => {
                                format!("Imported {}", candidate.file.name)
                            }
                            ImportedSubject::Environment { label } => {
                                format!("Using {label} as the scene environment")
                            }
                        };
                        emitter(ImportLifecycleEvent::Succeeded {
                            batch_id: batch.id,
                            index,
                            total,
                            file_name: candidate.file.name,
                            subject,
                            message,
                        });
                        break;
                    }
                    Ok(ImportAssetResult::Failed) => {
                        emitter(ImportLifecycleEvent::Failed {
                            batch_id: batch.id,
                            index,
                            total,
                            file_name: candidate.file.name.clone(),
                            message: format!(
                                "{} reached the importer but could not be read or decoded. Open Import Report for details.",
                                candidate.file.name
                            ),
                            recoverable: true,
                        });
                        break;
                    }
                    Ok(ImportAssetResult::Cancelled) => {
                        emit_cancelled(
                            &emitter,
                            batch.id,
                            index,
                            total,
                            &candidate.file.name,
                        );
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        emitter(ImportLifecycleEvent::Failed {
                            batch_id: batch.id,
                            index,
                            total,
                            file_name: candidate.file.name,
                            message: "The engine stopped before the import produced a result. Restart the editor, then try again."
                                .into(),
                            recoverable: true,
                        });
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let elapsed = started.elapsed();
                        if elapsed >= next_delayed {
                            // This is deliberately still progress, not failure. Engine commands are serial;
                            // dropping the receiver and claiming a timeout would let the model land later after
                            // the UI had already invited a duplicate retry.
                            emit_progress(
                                &emitter,
                                batch.id,
                                index,
                                total,
                                completed,
                                &candidate.file.name,
                                ImportProgressStage::Delayed,
                                elapsed,
                            );
                            next_delayed = elapsed.saturating_add(timing.delayed_repeat);
                        }
                    }
                }
            }
        }
        control.finish(batch.id);
    }
}

fn emit_cancelled(
    emitter: &LifecycleEmitter,
    batch_id: u64,
    index: usize,
    total: usize,
    file_name: &str,
) {
    emitter(ImportLifecycleEvent::Cancelled {
        batch_id,
        index,
        total,
        file_name: file_name.to_string(),
        message: format!(
            "Import of {file_name} stopped at a safe checkpoint. No scene changes were committed."
        ),
    });
}

#[allow(clippy::too_many_arguments)]
fn emit_progress(
    emitter: &LifecycleEmitter,
    batch_id: u64,
    index: usize,
    total: usize,
    completed: usize,
    file_name: &str,
    stage: ImportProgressStage,
    elapsed: Duration,
) {
    emitter(ImportLifecycleEvent::Progress {
        batch_id,
        index,
        total,
        completed,
        file_name: file_name.to_string(),
        stage,
        elapsed_ms: elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn candidate(path: &str, supported: bool) -> DropCandidate {
        let path = PathBuf::from(path);
        DropCandidate {
            file: DropFile {
                name: path.file_name().unwrap().to_string_lossy().into_owned(),
                supported,
                reason: (!supported).then(|| "This format is unavailable.".into()),
            },
            path,
        }
    }

    fn recording_emitter() -> (LifecycleEmitter, Arc<Mutex<Vec<ImportLifecycleEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&events);
        let emitter: LifecycleEmitter = Arc::new(move |event| recorded.lock().unwrap().push(event));
        (emitter, events)
    }

    fn batch(
        control: &NativeImportControl,
        id: u64,
        candidates: Vec<DropCandidate>,
    ) -> DropBatch {
        DropBatch {
            id,
            candidates,
            cancellation: control.begin(id),
        }
    }

    fn test_timing() -> WorkerTiming {
        WorkerTiming {
            reply_poll: Duration::from_millis(2),
            delayed_after: Duration::from_millis(5),
            delayed_repeat: Duration::from_secs(1),
        }
    }

    #[test]
    fn classifies_supported_step_and_explains_unknown_extension() {
        let step = classify_path(Path::new("analytic_trio.stp"));
        assert!(step.file.supported);
        assert_eq!(step.file.reason, None);

        let unknown = classify_path(Path::new("factory.not-a-format"));
        assert!(!unknown.file.supported);
        assert!(unknown.file.reason.as_deref().is_some_and(|reason| !reason.is_empty()));
    }

    #[test]
    fn lifecycle_payload_uses_camel_case_discriminants_and_fields() {
        let value = serde_json::to_value(ImportLifecycleEvent::Succeeded {
            batch_id: 7,
            index: 1,
            total: 1,
            file_name: "cell.stp".into(),
            subject: ImportedSubject::Entity {
                root_id: "entity-1".into(),
            },
            message: "Imported cell.stp".into(),
        })
        .unwrap();
        assert_eq!(value["phase"], "succeeded");
        assert_eq!(value["batchId"], 7);
        assert_eq!(value["fileName"], "cell.stp");
        assert_eq!(value["subject"]["rootId"], "entity-1");
    }

    #[test]
    fn full_queue_refuses_immediately_without_waiting() {
        let (batch_tx, _batch_rx) = mpsc::sync_channel(1);
        let control = NativeImportControl::default();
        batch_tx
            .try_send(batch(
                &control,
                1,
                vec![candidate("already.stp", true)],
            ))
            .unwrap();
        let (emitter, events) = recording_emitter();

        let started = Instant::now();
        dispatch_drop(
            &[PathBuf::from("next.stp")],
            &batch_tx,
            &emitter,
            &control,
        );
        assert!(started.elapsed() < Duration::from_millis(50));

        let events = events.lock().unwrap();
        assert!(matches!(events.first(), Some(ImportLifecycleEvent::Dropped { .. })));
        assert!(matches!(events.last(), Some(ImportLifecycleEvent::Failed { recoverable: true, .. })));
    }

    #[test]
    fn worker_routes_supported_file_through_import_asset_and_preserves_reply() {
        let (batch_tx, batch_rx) = mpsc::sync_channel(1);
        let (engine_tx, engine_rx) = mpsc::channel();
        let (emitter, events) = recording_emitter();
        let control = NativeImportControl::default();
        let worker_control = control.clone();
        let worker = std::thread::spawn(move || {
            run_worker(
                batch_rx,
                engine_tx,
                emitter,
                worker_control,
                test_timing(),
            );
        });

        batch_tx
            .send(batch(
                &control,
                41,
                vec![candidate("analytic_trio.stp", true)],
            ))
            .unwrap();
        let EngineCmd::ImportAsset {
            path,
            cancellation,
            reply,
        } = engine_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the worker must use the canonical engine command")
        else {
            panic!("drop bypassed EngineCmd::ImportAsset");
        };
        assert!(path.ends_with("analytic_trio.stp"));
        assert!(cancellation.is_some());
        reply
            .send(ImportAssetResult::Imported("root-entity".into()))
            .unwrap();

        for _ in 0..100 {
            if events
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, ImportLifecycleEvent::Succeeded { .. }))
            {
                break;
            }
            std::thread::yield_now();
        }
        drop(batch_tx);
        worker.join().unwrap();
        let events = events.lock().unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            ImportLifecycleEvent::Progress {
                stage: ImportProgressStage::Queued,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            ImportLifecycleEvent::Progress {
                stage: ImportProgressStage::Importing,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            ImportLifecycleEvent::Succeeded {
                subject: ImportedSubject::Entity { root_id },
                ..
            } if root_id == "root-entity"
        )));
    }

    #[test]
    fn a_slow_engine_emits_delayed_progress_not_false_failure() {
        let (batch_tx, batch_rx) = mpsc::sync_channel(1);
        let (engine_tx, engine_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let emitter: LifecycleEmitter = Arc::new(move |event| {
            let _ = event_tx.send(event);
        });
        let control = NativeImportControl::default();
        let worker_control = control.clone();
        let worker = std::thread::spawn(move || {
            run_worker(
                batch_rx,
                engine_tx,
                emitter,
                worker_control,
                test_timing(),
            );
        });

        batch_tx
            .send(batch(
                &control,
                42,
                vec![candidate("large_factory.3dxml", true)],
            ))
            .unwrap();
        let EngineCmd::ImportAsset { reply, .. } = engine_rx.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("drop bypassed EngineCmd::ImportAsset");
        };

        let mut saw_delayed = false;
        for _ in 0..10 {
            let event = event_rx.recv_timeout(Duration::from_millis(50)).unwrap();
            assert!(!matches!(event, ImportLifecycleEvent::Failed { .. }));
            if matches!(
                event,
                ImportLifecycleEvent::Progress {
                    stage: ImportProgressStage::Delayed,
                    ..
                }
            ) {
                saw_delayed = true;
                break;
            }
        }
        assert!(saw_delayed, "a long import stays visibly in progress");
        reply.send(ImportAssetResult::Failed).unwrap();
        drop(batch_tx);
        worker.join().unwrap();
    }

    #[test]
    fn queued_batch_cancels_without_reaching_engine_and_a_fresh_retry_succeeds() {
        let (batch_tx, batch_rx) = mpsc::sync_channel(2);
        let (engine_tx, engine_rx) = mpsc::channel();
        let (emitter, events) = recording_emitter();
        let control = NativeImportControl::default();

        let cancelled = batch(
            &control,
            51,
            vec![candidate("cancel-before-start.step", true)],
        );
        assert!(control.cancel(51), "an open queued batch accepts cancellation");
        batch_tx.send(cancelled).unwrap();

        let retry = batch(
            &control,
            52,
            vec![candidate("retry-after-cancel.step", true)],
        );
        batch_tx.send(retry).unwrap();

        let worker_control = control.clone();
        let worker = std::thread::spawn(move || {
            run_worker(
                batch_rx,
                engine_tx,
                emitter,
                worker_control,
                test_timing(),
            );
        });

        let EngineCmd::ImportAsset { path, reply, .. } = engine_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("only the retry should reach the engine")
        else {
            panic!("drop bypassed EngineCmd::ImportAsset");
        };
        assert!(path.ends_with("retry-after-cancel.step"));
        assert!(engine_rx.try_recv().is_err());
        reply
            .send(ImportAssetResult::Imported("retry-root".into()))
            .unwrap();
        drop(batch_tx);
        worker.join().unwrap();

        let events = events.lock().unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            ImportLifecycleEvent::Cancelled { batch_id: 51, .. }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            ImportLifecycleEvent::Succeeded { batch_id: 52, .. }
        )));
        assert!(!control.contains(51));
        assert!(!control.contains(52));
    }

    #[test]
    fn current_batch_cancellation_signals_the_engine_owned_atomic_and_emits_terminal_cancelled() {
        let (batch_tx, batch_rx) = mpsc::sync_channel(1);
        let (engine_tx, engine_rx) = mpsc::channel();
        let (emitter, events) = recording_emitter();
        let control = NativeImportControl::default();
        let worker_control = control.clone();
        let worker = std::thread::spawn(move || {
            run_worker(
                batch_rx,
                engine_tx,
                emitter,
                worker_control,
                test_timing(),
            );
        });

        batch_tx
            .send(batch(
                &control,
                53,
                vec![candidate("cancel-during-parse.3dxml", true)],
            ))
            .unwrap();
        let EngineCmd::ImportAsset {
            cancellation,
            reply,
            ..
        } = engine_rx.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("drop bypassed EngineCmd::ImportAsset");
        };
        let engine_token = cancellation.expect("native drops carry the shared token");
        assert!(control.cancel(53));
        assert!(
            engine_token.is_requested(),
            "the direct control plane reaches the in-flight engine token"
        );
        reply.send(ImportAssetResult::Cancelled).unwrap();
        drop(batch_tx);
        worker.join().unwrap();

        assert!(events.lock().unwrap().iter().any(|event| matches!(
            event,
            ImportLifecycleEvent::Cancelled {
                batch_id: 53,
                message,
                ..
            } if message.contains("safe checkpoint") && message.contains("No scene changes")
        )));
        assert!(!control.contains(53));
    }

    #[test]
    fn unsupported_file_is_refused_without_reaching_the_engine() {
        let (batch_tx, batch_rx) = mpsc::sync_channel(1);
        let (engine_tx, engine_rx) = mpsc::channel();
        let (emitter, events) = recording_emitter();
        let control = NativeImportControl::default();
        let worker_control = control.clone();
        let worker = std::thread::spawn(move || {
            run_worker(
                batch_rx,
                engine_tx,
                emitter,
                worker_control,
                test_timing(),
            );
        });
        batch_tx
            .send(batch(
                &control,
                43,
                vec![candidate("notes.txt", false)],
            ))
            .unwrap();
        drop(batch_tx);
        worker.join().unwrap();

        assert!(matches!(engine_rx.try_recv(), Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected)));
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, ImportLifecycleEvent::Refused { .. })));
    }
}
