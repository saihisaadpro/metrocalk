//! Deterministic, project-owned animation domain kernel.
//!
//! This crate owns animation time, typed bindings, authored sequences, deterministic compilation and
//! sampling, event crossings, and self-describing asset metadata. It intentionally has no renderer,
//! ECS, document, glTF, or platform dependency, so the same validated asset can drive editor preview,
//! game runtime, CAD motion studies, server simulation, and `wasm32-unknown-unknown`.

#![forbid(unsafe_code)]

mod binding;
mod compile;
mod domain;
pub mod graph;
mod instance;
mod manifest;
pub mod preset;
mod runtime;
mod time;
mod value;

pub use binding::{
    AnimationBindingContext, AnimationBindingDescriptor, AnimationBindingDiagnostic,
    AnimationBindingDiagnosticCode, AnimationDomain, AnimationEditorKind, AnimationReadiness,
    AnimationRuntimeSink, AnimationSemantic,
};
pub use compile::{
    CompileError, CompiledSequence, Direction, EvaluatedBinding, Evaluation, EventBatch,
    EventOccurrence,
};
pub use domain::{
    AnimationEvent, Binding, Clip, ClipId, EventId, Interpolation, IssueCode, KeyId, Keyframe,
    LoopPolicy, Marker, MarkerId, PropertyPath, Sequence, SequenceId, Severity, Track, TrackId,
    ValidationIssue,
};
pub use graph::*;
pub use instance::{
    instantiate_clip, BindingRebind, BindingSignature, ClipInstanceSource, ClipInstantiationCode,
    ClipInstantiationError, ClipInstantiationIssue, ClipInstantiationRequest, CompiledClipInstance,
    TargetRebind, UnmappedBindingPolicy, BINDING_SIGNATURE_VERSION,
};
pub use manifest::*;
pub use preset::{
    fade, loading_pulse, rigid_camera_move, sprite_loop, ui_click, ui_hover, AnimationPreset,
};
pub use runtime::{
    AnimationBindingWrite, AnimationFramePatch, AnimationRebindPolicy, AnimationRuntimeInstance,
    AnimationUpdateMode, DEFAULT_RUNTIME_EVENT_LIMIT, MAX_RUNTIME_EVENT_LIMIT,
};
pub use time::{Tick, TimeBase, TimeError};
pub use value::{AnimValue, ValueKind};

/// Current authored-sequence schema version.
pub const SEQUENCE_SCHEMA_VERSION: u32 = 1;

#[cfg(test)]
mod tests;
