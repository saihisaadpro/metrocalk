//! Animation authoring adapter — maps the pure animation kernel onto the one authoritative ECS/Loro
//! op-stream. Track metadata and every keyframe occupy independent component fields, so editing one key
//! does not replace a whole clip and concurrent same-tick keys converge by stable `KeyId`.
//!
//! Evaluation is deliberately read-only. The native host consumes [`AnimationDocument::compile`] into a
//! render projection; sampled values are never committed back into authored component fields.
#![expect(
    clippy::needless_pass_by_value,
    clippy::too_many_arguments,
    reason = "an authoring entry point takes the authored document by value and names every field it               accepts; collapsing them into a struct would only move the same arity behind a name"
)]
#![expect(
    clippy::too_many_lines,
    reason = "one linear validation pass per document; the order of the checks is the contract"
)]
use std::collections::{BTreeMap, BTreeSet};

use metrocalk_animation::{
    AnimValue, AnimationBindingContext, AnimationEvent, AnimationReadiness, Binding,
    CompiledSequence, EventId, Interpolation, Keyframe, LoopPolicy, Marker, MarkerId, PropertyPath,
    Sequence, SequenceId, Tick, TimeBase, Track, TrackId, ValueKind,
};
use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::animation_bindings::standard_animation_binding_registry;
use crate::animation_graph_intent::ANIMATION_GRAPH;
use crate::kinematics::{joint_of_with_component, parse_track, JOINT_TRACK};

pub const ANIMATION_TRACK: &str = "AnimationTrack";
pub const ANIMATION_TRACK_SCHEMA_VERSION: u32 = 2;
const LEGACY_ANIMATION_TRACK_SCHEMA_VERSION: u32 = 1;
pub const MAIN_SEQUENCE_ID: &str = "main";
pub const DEFAULT_TICKS_PER_SECOND: u32 = 60_000;
const TRACK_FIELD_PREFIX: &str = "track::";
const KEY_FIELD_PREFIX: &str = "key::";
const MARKER_FIELD_PREFIX: &str = "marker::";
const EVENT_FIELD_PREFIX: &str = "event::";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct StoredTrackMeta {
    schema_version: u32,
    id: TrackId,
    name: String,
    binding: Binding,
    interpolation: Interpolation,
    enabled: bool,
}

impl StoredTrackMeta {
    #[cfg(test)]
    fn from_track(track: &Track) -> Self {
        Self {
            schema_version: LEGACY_ANIMATION_TRACK_SCHEMA_VERSION,
            id: track.id.clone(),
            name: track.name.clone(),
            binding: track.binding.clone(),
            interpolation: track.interpolation,
            enabled: track.enabled,
        }
    }
}

#[derive(Clone, Debug)]
struct TrackSeed {
    name: String,
    component: String,
    property: String,
    value_kind: ValueKind,
    interpolation: Interpolation,
    enabled: bool,
    locked: bool,
}

impl TrackSeed {
    fn from_legacy(meta: &StoredTrackMeta) -> Self {
        Self {
            name: meta.name.clone(),
            component: meta.binding.path.component.clone(),
            property: meta.binding.path.property.clone(),
            value_kind: meta.binding.value_kind,
            interpolation: meta.interpolation,
            enabled: meta.enabled,
            locked: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnimatableProperty {
    pub component: String,
    pub property: String,
    pub label: String,
    pub value_kind: Option<ValueKind>,
    pub value: FieldValue,
    pub animatable: bool,
    pub reason: Option<String>,
    pub binding: AnimationBindingContext,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimationStorageIssue {
    pub severity: &'static str,
    pub code: &'static str,
    pub message: String,
    pub remediation: Option<String>,
    pub target: Option<EntityId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnimationDocument {
    pub sequence: Sequence,
    pub storage_issues: Vec<AnimationStorageIssue>,
    /// Editor locks are durable collaboration metadata, not a runtime sampling concern.
    pub locked_tracks: BTreeSet<TrackId>,
    pub marker_owners: BTreeMap<MarkerId, EntityId>,
    pub event_owners: BTreeMap<EventId, EntityId>,
}

impl AnimationDocument {
    pub fn compile(&self) -> Result<CompiledSequence, metrocalk_animation::CompileError> {
        self.sequence.compile()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimationKeyReceipt {
    pub track_id: String,
    pub key_id: String,
    pub replaced_keys: usize,
}

/// One fully-qualified authored key selected for an atomic multi-track edit.
///
/// Key IDs are peer-namespaced, but legacy and compatibility tracks can still reuse textual IDs. The
/// owning entity and public track ID therefore remain part of the identity at every mutation boundary.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AnimationKeyDelete {
    pub entity: EntityId,
    pub track_id: String,
    pub key_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnimationIntentError {
    MissingEntity,
    MissingProperty,
    ProtectedProperty(String),
    InvalidTick,
    InvalidValue,
    MissingTrack,
    MissingKey,
    LockedTrack,
    CorruptTrack(String),
    Commit(String),
}

impl std::fmt::Display for AnimationIntentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEntity => formatter.write_str("the animation target no longer exists"),
            Self::MissingProperty => {
                formatter.write_str("the property no longer exists on the target")
            }
            Self::ProtectedProperty(reason) => formatter.write_str(reason),
            Self::InvalidTick => {
                formatter.write_str("key time must be a non-negative integer tick")
            }
            Self::InvalidValue => {
                formatter.write_str("the current property value cannot be animated")
            }
            Self::MissingTrack => formatter.write_str("the animation track no longer exists"),
            Self::MissingKey => formatter.write_str("the animation key no longer exists"),
            Self::LockedTrack => formatter.write_str(
                "the animation track is locked; unlock it before changing keys or interpolation",
            ),
            Self::CorruptTrack(reason) => {
                write!(formatter, "the animation track is damaged: {reason}")
            }
            Self::Commit(reason) => write!(formatter, "the animation edit was rejected: {reason}"),
        }
    }
}

impl std::error::Error for AnimationIntentError {}

/// The selected entity's real scalar component fields, including protected entries with an explanation.
/// This is a live catalog rather than a second schema that can drift from authored data.
#[must_use]
pub fn property_catalog(engine: &Engine<FlecsWorld>, entity: EntityId) -> Vec<AnimatableProperty> {
    if !engine.entity_ids().contains(&entity) {
        return Vec::new();
    }
    let legacy_kinematic_component = joint_of_with_component(engine, entity).map(|(_, name)| name);
    let registry = standard_animation_binding_registry();
    let target = entity.to_loro_key();
    let mut out = Vec::new();
    // Key the resolved value the viewport presents (base plus instance overrides), not a hidden base value.
    for (component, fields) in engine.resolved_components(entity) {
        for (property, value) in fields {
            let kind = value_kind(&value);
            let binding = registry.resolve(&PropertyPath::new(&target, &component, &property));
            let reason = protection_reason(
                &component,
                &property,
                &value,
                legacy_kinematic_component,
                &binding,
            );
            out.push(AnimatableProperty {
                label: format!(
                    "{} / {}",
                    display_token(&component),
                    display_token(&property)
                ),
                component: component.clone(),
                property,
                value_kind: Some(kind),
                value,
                animatable: reason.is_none(),
                reason,
                binding,
            });
        }
    }
    out.sort_by(|left, right| {
        left.component
            .cmp(&right.component)
            .then(left.property.cmp(&right.property))
    });
    out
}

/// Add or replace a key at one property/tick. Metadata and the key are separate fields in one undoable
/// transaction. Existing same-tick keys from this replica are removed; a concurrent peer's independently
/// named key survives and the kernel resolves it by `(tick, KeyId)` deterministically.
pub fn key_property(
    engine: &mut Engine<FlecsWorld>,
    entity: EntityId,
    component: &str,
    property: &str,
    tick: Tick,
    requested_interpolation: Interpolation,
) -> Result<AnimationKeyReceipt, AnimationIntentError> {
    if tick.0 < 0 {
        return Err(AnimationIntentError::InvalidTick);
    }
    let catalog = property_catalog(engine, entity);
    if catalog.is_empty() && !engine.entity_ids().contains(&entity) {
        return Err(AnimationIntentError::MissingEntity);
    }
    let fact = catalog
        .iter()
        .find(|item| item.component == component && item.property == property)
        .ok_or(AnimationIntentError::MissingProperty)?;
    if !fact.animatable {
        return Err(AnimationIntentError::ProtectedProperty(
            fact.reason
                .clone()
                .unwrap_or_else(|| "this property is not safely animatable".into()),
        ));
    }
    let value = anim_value(&fact.value);
    if !value.is_finite() {
        return Err(AnimationIntentError::InvalidValue);
    }
    let interpolation = if value.is_interpolatable() {
        requested_interpolation
    } else {
        Interpolation::Step
    };
    let target = entity.to_loro_key();
    let track_id = stable_track_id(&target, component, property);
    let document = load_animation_document(engine);
    let existing_track = document
        .sequence
        .tracks
        .iter()
        .find(|existing| existing.id.as_str() == track_id);
    if document
        .locked_tracks
        .contains(&TrackId::new(track_id.clone()))
    {
        return Err(AnimationIntentError::LockedTrack);
    }
    // A copied composition may still carry the source entity's storage prefix. Public identity is
    // canonical to this target/property; storage identity is resolved only at the persistence edge.
    let storage_track_id =
        resolve_storage_track_id(engine, entity, &track_id).unwrap_or_else(|| track_id.clone());
    let same_tick: Vec<_> = existing_track
        .into_iter()
        .flat_map(|track| track.keyframes.iter())
        .filter(|key| key.tick == tick)
        .collect();
    let local_same_tick: Vec<_> = same_tick
        .iter()
        .copied()
        .filter(|key| generated_element_peer(key.id.as_str(), "key") == Some(engine.peer_id()))
        .collect();
    // Re-keying an existing time updates that element in place. A new key receives a generated-once,
    // peer-namespaced identity that remains stable when its time/value/tangents later change. Concurrent
    // peers' same-tick elements are independent CRDT choices: this peer must never overwrite or tombstone
    // them merely because they occupy the same time.
    let key_id = local_same_tick.first().map_or_else(
        || allocate_element_id(engine, "key"),
        |existing| existing.id.as_str().to_owned(),
    );
    let track = Track {
        id: TrackId::new(track_id.clone()),
        name: format!("{}.{}", display_token(component), display_token(property)),
        binding: Binding {
            path: PropertyPath::new(&target, component, property),
            value_kind: value.kind(),
        },
        interpolation,
        keyframes: Vec::new(),
        enabled: true,
    };
    let key = Keyframe::new(key_id.as_str(), tick, value);
    let fields = animation_fields(engine, entity);
    let mut ops = Vec::new();
    if !fields.contains_key(&track_field(&storage_track_id))
        && !fields
            .keys()
            .any(|field| field.starts_with(&format!("{TRACK_FIELD_PREFIX}{storage_track_id}::")))
    {
        ops.extend(track_create_ops(entity, &track));
    }
    for existing_key in &local_same_tick {
        if existing_key.id.as_str() != key_id {
            ops.push(set_animation_field(
                entity,
                key_semantic_field(&storage_track_id, existing_key.id.as_str(), "active"),
                FieldValue::Bool(false),
            ));
        }
    }
    let replaced_keys = local_same_tick.len();
    ops.extend(key_create_ops(entity, &storage_track_id, &key)?);
    engine
        .commit("animation-key", ops)
        .map_err(|error| AnimationIntentError::Commit(error.to_string()))?;
    Ok(AnimationKeyReceipt {
        track_id,
        key_id,
        replaced_keys,
    })
}

/// Author a continuous yaw spin (360° per `period_ticks`, loop-seamless) as **ops for the caller's
/// own transaction** — the gameplay-roles path, where the spin must land in the SAME commit as the
/// role's components so one Ctrl-Z removes everything. Unlike [`key_property`] (which keys the value
/// currently in the committed doc), the quarter-turn quaternion values here are explicit, so the ops
/// are valid inside a batch that also creates fields.
///
/// Idempotent: if either spin track already exists on the entity, returns an empty op list (the spin
/// is already authored — re-assigning a role never duplicates tracks).
///
/// # Errors
/// [`AnimationIntentError::InvalidTick`] for a non-positive period; key-encoding failures.
#[allow(clippy::similar_names)] // qy/qw ARE the quaternion channel names — renaming them would obscure
pub fn author_spin_ops(
    engine: &mut Engine<FlecsWorld>,
    entity: EntityId,
    period_ticks: i64,
) -> Result<Vec<Op>, AnimationIntentError> {
    if period_ticks <= 0 {
        return Err(AnimationIntentError::InvalidTick);
    }
    let target = entity.to_loro_key();
    let fields = animation_fields(engine, entity);
    let track_exists = |track_id: &str| {
        fields.contains_key(&track_field(track_id))
            || fields
                .keys()
                .any(|field| field.starts_with(&format!("{TRACK_FIELD_PREFIX}{track_id}::")))
    };
    let qy_id = stable_track_id(&target, "Transform", "qy");
    let qw_id = stable_track_id(&target, "Transform", "qw");
    if track_exists(&qy_id) || track_exists(&qw_id) {
        return Ok(Vec::new());
    }
    // A full turn as five quarter-turn poses (a 2-key 0°→360° track would lerp through the origin and
    // collapse to identity); the wrap is seamless because (0,0,0,−1) ≡ (0,0,0,1) as rotations.
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let quarters: [(f64, f64); 5] = [(0.0, 1.0), (s, s), (1.0, 0.0), (s, -s), (0.0, -1.0)];
    let mut ops = Vec::new();
    for (property, track_id, pick) in [("qy", &qy_id, 0usize), ("qw", &qw_id, 1usize)] {
        let track = Track {
            id: TrackId::new(track_id.clone()),
            name: format!("{}.{}", display_token("Transform"), display_token(property)),
            binding: Binding {
                path: PropertyPath::new(&target, "Transform", property),
                value_kind: AnimValue::Number(0.0).kind(),
            },
            interpolation: Interpolation::Linear,
            keyframes: Vec::new(),
            enabled: true,
        };
        ops.extend(track_create_ops(entity, &track));
        for (i, pose) in quarters.iter().enumerate() {
            let tick = Tick(period_ticks * i64::try_from(i).unwrap_or(4) / 4);
            let value = AnimValue::Number(if pick == 0 { pose.0 } else { pose.1 });
            let key = Keyframe::new(allocate_element_id(engine, "key").as_str(), tick, value);
            ops.extend(key_create_ops(entity, track_id, &key)?);
        }
    }
    Ok(ops)
}

pub fn delete_key(
    engine: &mut Engine<FlecsWorld>,
    entity: EntityId,
    track_id: &str,
    key_id: &str,
) -> Result<(), AnimationIntentError> {
    delete_keys(
        engine,
        &[AnimationKeyDelete {
            entity,
            track_id: track_id.to_owned(),
            key_id: key_id.to_owned(),
        }],
    )
    .map(|_| ())
}

/// Tombstone keys across any number of entities and tracks as one validated, undoable transaction.
///
/// Every key is resolved and checked before the first operation is committed. A stale key, missing
/// entity, or locked track therefore rejects the whole selection instead of leaving a partial deletion.
pub fn delete_keys(
    engine: &mut Engine<FlecsWorld>,
    deletes: &[AnimationKeyDelete],
) -> Result<usize, AnimationIntentError> {
    if deletes.is_empty() {
        return Ok(0);
    }
    let document = load_animation_document(engine);
    let entities: BTreeSet<_> = engine.entity_ids().into_iter().collect();
    let unique: BTreeSet<_> = deletes.iter().cloned().collect();
    let mut resolved = Vec::with_capacity(unique.len());
    for delete in unique {
        if !entities.contains(&delete.entity) {
            return Err(AnimationIntentError::MissingEntity);
        }
        let public_track_id = TrackId::new(delete.track_id.clone());
        let track = document
            .sequence
            .tracks
            .iter()
            .find(|track| {
                track.id == public_track_id
                    && track.binding.path.target == delete.entity.to_loro_key()
            })
            .ok_or(AnimationIntentError::MissingTrack)?;
        if document.locked_tracks.contains(&public_track_id) {
            return Err(AnimationIntentError::LockedTrack);
        }
        if !track
            .keyframes
            .iter()
            .any(|key| key.id.as_str() == delete.key_id)
        {
            return Err(AnimationIntentError::MissingKey);
        }
        let storage_track_id = resolve_storage_track_id(engine, delete.entity, &delete.track_id)
            .ok_or(AnimationIntentError::MissingTrack)?;
        let fields = animation_fields(engine, delete.entity);
        let legacy_field = key_field(&storage_track_id, &delete.key_id);
        let semantic_prefix = format!("{legacy_field}::");
        if !fields.contains_key(&legacy_field)
            && !fields
                .keys()
                .any(|field| field.starts_with(&semantic_prefix))
        {
            return Err(AnimationIntentError::MissingKey);
        }
        resolved.push((delete, storage_track_id));
    }

    let count = resolved.len();
    let ops = resolved
        .into_iter()
        .map(|(delete, storage_track_id)| {
            set_animation_field(
                delete.entity,
                key_semantic_field(&storage_track_id, &delete.key_id, "active"),
                FieldValue::Bool(false),
            )
        })
        .collect();
    engine
        .commit("animation-delete-keys", ops)
        .map_err(|error| AnimationIntentError::Commit(error.to_string()))?;
    Ok(count)
}

pub fn set_track_interpolation(
    engine: &mut Engine<FlecsWorld>,
    entity: EntityId,
    track_id: &str,
    interpolation: Interpolation,
) -> Result<(), AnimationIntentError> {
    let document = load_animation_document(engine);
    if document
        .locked_tracks
        .contains(&TrackId::new(track_id.to_owned()))
    {
        return Err(AnimationIntentError::LockedTrack);
    }
    let track = document
        .sequence
        .tracks
        .into_iter()
        .find(|track| track.id.as_str() == track_id)
        .ok_or(AnimationIntentError::MissingTrack)?;
    if matches!(
        track.binding.value_kind,
        ValueKind::Boolean | ValueKind::String
    ) && interpolation != Interpolation::Step
    {
        return Err(AnimationIntentError::ProtectedProperty(
            "boolean and text tracks use Step interpolation because intermediate values do not exist"
                .into(),
        ));
    }
    let storage_track_id = resolve_storage_track_id(engine, entity, track_id)
        .ok_or(AnimationIntentError::MissingTrack)?;
    engine
        .commit(
            "animation-interpolation",
            vec![Op::SetField {
                entity,
                component: ANIMATION_TRACK.into(),
                field: track_semantic_field(&storage_track_id, "interpolation"),
                value: FieldValue::Str(interpolation_name(interpolation).into()),
            }],
        )
        .map_err(|error| AnimationIntentError::Commit(error.to_string()))
}

/// Mute/unmute a track without deleting authored keys. This is runtime data, so it is stored beside
/// interpolation and participates in undo/redo and collaboration.
pub fn set_track_enabled(
    engine: &mut Engine<FlecsWorld>,
    entity: EntityId,
    track_id: &str,
    enabled: bool,
) -> Result<(), AnimationIntentError> {
    let document = load_animation_document(engine);
    if document
        .locked_tracks
        .contains(&TrackId::new(track_id.to_owned()))
    {
        return Err(AnimationIntentError::LockedTrack);
    }
    if !document
        .sequence
        .tracks
        .iter()
        .any(|track| track.id.as_str() == track_id)
    {
        return Err(AnimationIntentError::MissingTrack);
    }
    let storage_track_id = resolve_storage_track_id(engine, entity, track_id)
        .ok_or(AnimationIntentError::MissingTrack)?;
    engine
        .commit(
            "animation-track-enabled",
            vec![set_animation_field(
                entity,
                track_semantic_field(&storage_track_id, "enabled"),
                FieldValue::Bool(enabled),
            )],
        )
        .map_err(|error| AnimationIntentError::Commit(error.to_string()))
}

/// Locks are durable editor collaboration metadata. Locking never changes sampling; it prevents later
/// authoring commands from mutating the track until explicitly unlocked.
pub fn set_track_locked(
    engine: &mut Engine<FlecsWorld>,
    entity: EntityId,
    track_id: &str,
    locked: bool,
) -> Result<(), AnimationIntentError> {
    if !load_animation_document(engine)
        .sequence
        .tracks
        .iter()
        .any(|track| track.id.as_str() == track_id)
    {
        return Err(AnimationIntentError::MissingTrack);
    }
    let storage_track_id = resolve_storage_track_id(engine, entity, track_id)
        .ok_or(AnimationIntentError::MissingTrack)?;
    engine
        .commit(
            "animation-track-locked",
            vec![set_animation_field(
                entity,
                track_semantic_field(&storage_track_id, "locked"),
                FieldValue::Bool(locked),
            )],
        )
        .map_err(|error| AnimationIntentError::Commit(error.to_string()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnimationKeyUpdate {
    pub key_id: String,
    pub tick: Tick,
    pub value: Option<AnimValue>,
    pub in_tangent: Option<Option<AnimValue>>,
    pub out_tangent: Option<Option<AnimValue>>,
}

/// Update multiple keys as one transaction (the timeline drag contract). Stable IDs are retained and
/// no intermediate pointer-move writes enter undo history.
pub fn update_keys(
    engine: &mut Engine<FlecsWorld>,
    entity: EntityId,
    track_id: &str,
    updates: &[AnimationKeyUpdate],
) -> Result<(), AnimationIntentError> {
    let document = load_animation_document(engine);
    if document
        .locked_tracks
        .contains(&TrackId::new(track_id.to_owned()))
    {
        return Err(AnimationIntentError::LockedTrack);
    }
    let track = document
        .sequence
        .tracks
        .iter()
        .find(|track| track.id.as_str() == track_id)
        .ok_or(AnimationIntentError::MissingTrack)?;
    let storage_track_id = resolve_storage_track_id(engine, entity, track_id)
        .ok_or(AnimationIntentError::MissingTrack)?;
    let mut seen = BTreeSet::new();
    let mut ops = Vec::new();
    for update in updates {
        if update.tick.0 < 0 {
            return Err(AnimationIntentError::InvalidTick);
        }
        if !seen.insert(update.key_id.as_str()) {
            return Err(AnimationIntentError::CorruptTrack(format!(
                "key {} appears more than once in one edit",
                update.key_id
            )));
        }
        let key = track
            .keyframes
            .iter()
            .find(|key| key.id.as_str() == update.key_id)
            .ok_or(AnimationIntentError::MissingKey)?;
        if let Some(value) = &update.value {
            if value.kind() != track.binding.value_kind || !value.is_finite() {
                return Err(AnimationIntentError::InvalidValue);
            }
            ops.push(set_animation_field(
                entity,
                key_semantic_field(&storage_track_id, &update.key_id, "value"),
                FieldValue::Str(encode(value)?),
            ));
        }
        for (name, tangent) in [
            ("in_tangent", update.in_tangent.as_ref()),
            ("out_tangent", update.out_tangent.as_ref()),
        ] {
            if let Some(tangent) = tangent {
                if tangent
                    .as_ref()
                    .is_some_and(|value| value.kind() != key.value.kind() || !value.is_finite())
                {
                    return Err(AnimationIntentError::InvalidValue);
                }
                ops.push(set_animation_field(
                    entity,
                    key_semantic_field(&storage_track_id, &update.key_id, name),
                    FieldValue::Str(encode(tangent)?),
                ));
            }
        }
        ops.push(set_animation_field(
            entity,
            key_semantic_field(&storage_track_id, &update.key_id, "tick"),
            FieldValue::Integer(update.tick.0),
        ));
    }
    if ops.is_empty() {
        return Ok(());
    }
    engine
        .commit("animation-update-keys", ops)
        .map_err(|error| AnimationIntentError::Commit(error.to_string()))
}

pub fn add_marker(
    engine: &mut Engine<FlecsWorld>,
    owner: EntityId,
    name: &str,
    tick: Tick,
    color: Option<[u8; 4]>,
) -> Result<String, AnimationIntentError> {
    if !engine.entity_ids().contains(&owner) {
        return Err(AnimationIntentError::MissingEntity);
    }
    if tick.0 < 0 {
        return Err(AnimationIntentError::InvalidTick);
    }
    let id = allocate_element_id(engine, "marker");
    let values = [
        ("active", FieldValue::Bool(true)),
        ("name", FieldValue::Str(non_empty_name(name, "Marker"))),
        ("tick", FieldValue::Integer(tick.0)),
        ("color", FieldValue::Str(encode(&color)?)),
    ];
    engine
        .commit(
            "animation-add-marker",
            values
                .into_iter()
                .map(|(field, value)| {
                    set_animation_field(owner, marker_semantic_field(&id, field), value)
                })
                .collect(),
        )
        .map_err(|error| AnimationIntentError::Commit(error.to_string()))?;
    Ok(canonical_record_id(owner, "marker", &id))
}

pub fn delete_marker(
    engine: &mut Engine<FlecsWorld>,
    owner: EntityId,
    marker_id: &str,
) -> Result<(), AnimationIntentError> {
    let storage_id = resolve_record_id(engine, owner, MARKER_FIELD_PREFIX, "marker", marker_id)
        .ok_or(AnimationIntentError::MissingKey)?;
    if !record_exists(engine, owner, MARKER_FIELD_PREFIX, &storage_id) {
        return Err(AnimationIntentError::MissingKey);
    }
    engine
        .commit(
            "animation-delete-marker",
            vec![set_animation_field(
                owner,
                marker_semantic_field(&storage_id, "active"),
                FieldValue::Bool(false),
            )],
        )
        .map_err(|error| AnimationIntentError::Commit(error.to_string()))
}

pub fn add_event(
    engine: &mut Engine<FlecsWorld>,
    owner: EntityId,
    name: &str,
    tick: Tick,
    payload: Option<AnimValue>,
) -> Result<String, AnimationIntentError> {
    if !engine.entity_ids().contains(&owner) {
        return Err(AnimationIntentError::MissingEntity);
    }
    if tick.0 < 0 {
        return Err(AnimationIntentError::InvalidTick);
    }
    if payload.as_ref().is_some_and(|value| !value.is_finite()) {
        return Err(AnimationIntentError::InvalidValue);
    }
    let id = allocate_element_id(engine, "event");
    let values = [
        ("active", FieldValue::Bool(true)),
        ("name", FieldValue::Str(non_empty_name(name, "Event"))),
        ("tick", FieldValue::Integer(tick.0)),
        ("payload", FieldValue::Str(encode(&payload)?)),
    ];
    engine
        .commit(
            "animation-add-event",
            values
                .into_iter()
                .map(|(field, value)| {
                    set_animation_field(owner, event_semantic_field(&id, field), value)
                })
                .collect(),
        )
        .map_err(|error| AnimationIntentError::Commit(error.to_string()))?;
    Ok(canonical_record_id(owner, "event", &id))
}

pub fn delete_event(
    engine: &mut Engine<FlecsWorld>,
    owner: EntityId,
    event_id: &str,
) -> Result<(), AnimationIntentError> {
    let storage_id = resolve_record_id(engine, owner, EVENT_FIELD_PREFIX, "event", event_id)
        .ok_or(AnimationIntentError::MissingKey)?;
    if !record_exists(engine, owner, EVENT_FIELD_PREFIX, &storage_id) {
        return Err(AnimationIntentError::MissingKey);
    }
    engine
        .commit(
            "animation-delete-event",
            vec![set_animation_field(
                owner,
                event_semantic_field(&storage_id, "active"),
                FieldValue::Bool(false),
            )],
        )
        .map_err(|error| AnimationIntentError::Commit(error.to_string()))
}

/// Fingerprint the complete authored animation components, including metadata that compilation omits.
///
/// Compiled-plan hashes intentionally ignore disabled tracks, editor locks and inactive key tombstones.
/// Those fields still change the document and must invalidate editor read models, so workspace revision
/// identity is derived from sorted raw animation fields. Animation graphs participate because graph and
/// timeline are one authored workspace. Legacy `JointTrack` fields participate until their compatibility
/// timelines have been migrated into universal tracks.
#[must_use]
pub fn authored_animation_revision(engine: &Engine<FlecsWorld>) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    revision_hash_frame(&mut hash, b"metrocalk-authored-animation-v1");
    let mut entities = engine.entity_ids();
    entities.sort();
    for entity in entities {
        let components = engine.components_of(entity);
        let has_animation = [ANIMATION_TRACK, ANIMATION_GRAPH, JOINT_TRACK]
            .into_iter()
            .any(|component| {
                components
                    .get(component)
                    .is_some_and(|fields| !fields.is_empty())
            });
        if !has_animation {
            continue;
        }
        revision_hash_frame(&mut hash, b"entity");
        revision_hash_frame(&mut hash, entity.to_loro_key().as_bytes());
        for component in [ANIMATION_TRACK, ANIMATION_GRAPH, JOINT_TRACK] {
            let Some(fields) = components.get(component) else {
                continue;
            };
            revision_hash_frame(&mut hash, b"component");
            revision_hash_frame(&mut hash, component.as_bytes());
            let fields: BTreeMap<_, _> = fields.iter().collect();
            for (name, value) in fields {
                revision_hash_frame(&mut hash, b"field");
                revision_hash_frame(&mut hash, name.as_bytes());
                match value {
                    FieldValue::Integer(value) => {
                        revision_hash_frame(&mut hash, b"integer");
                        revision_hash_frame(&mut hash, &value.to_le_bytes());
                    }
                    FieldValue::Number(value) => {
                        revision_hash_frame(&mut hash, b"number");
                        revision_hash_frame(&mut hash, &value.to_bits().to_le_bytes());
                    }
                    FieldValue::Bool(value) => {
                        revision_hash_frame(&mut hash, b"bool");
                        revision_hash_frame(&mut hash, &[u8::from(*value)]);
                    }
                    FieldValue::Str(value) => {
                        revision_hash_frame(&mut hash, b"string");
                        revision_hash_frame(&mut hash, value.as_bytes());
                    }
                }
            }
        }
    }
    format!("authored-{hash:016x}")
}

/// Reconstruct one deterministic sequence from granular component fields. Legacy CAD `JointTrack` data
/// participates through a compatibility adapter until the user authors a universal track for that DOF.
#[must_use]
pub fn load_animation_document(engine: &Engine<FlecsWorld>) -> AnimationDocument {
    let mut tracks = Vec::new();
    let mut locked_tracks = BTreeSet::new();
    let mut markers = Vec::new();
    let mut marker_owners = BTreeMap::new();
    let mut events = Vec::new();
    let mut event_owners = BTreeMap::new();
    let mut storage_issues = Vec::new();
    let mut universal_bindings = BTreeSet::new();
    let mut max_tick = Tick(DEFAULT_TICKS_PER_SECOND.into());

    let mut entities = engine.entity_ids();
    entities.sort();
    for entity in &entities {
        load_authored_entity(
            engine,
            *entity,
            &mut tracks,
            &mut locked_tracks,
            &mut markers,
            &mut marker_owners,
            &mut events,
            &mut event_owners,
            &mut storage_issues,
            &mut universal_bindings,
            &mut max_tick,
        );
    }

    // Existing CAD mechanism timelines remain playable and appear in the universal workspace. Once a
    // granular track exists for the same exact binding, it owns evaluation and the adapter stays silent.
    for entity in &entities {
        append_legacy_joint_track(
            engine,
            *entity,
            &universal_bindings,
            &mut tracks,
            &mut max_tick,
        );
    }

    tracks.sort_by(|left, right| {
        left.binding
            .cmp(&right.binding)
            .then(left.id.cmp(&right.id))
    });
    AnimationDocument {
        sequence: Sequence {
            schema_version: metrocalk_animation::SEQUENCE_SCHEMA_VERSION,
            id: SequenceId::new(MAIN_SEQUENCE_ID),
            name: "Main sequence".into(),
            time_base: TimeBase::new(DEFAULT_TICKS_PER_SECOND),
            duration: max_tick,
            loop_policy: LoopPolicy::Once,
            tracks,
            clips: Vec::new(),
            markers,
            events,
        },
        storage_issues,
        locked_tracks,
        marker_owners,
        event_owners,
    }
}

fn load_authored_entity(
    engine: &Engine<FlecsWorld>,
    entity: EntityId,
    tracks: &mut Vec<Track>,
    locked_tracks: &mut BTreeSet<TrackId>,
    markers: &mut Vec<Marker>,
    marker_owners: &mut BTreeMap<MarkerId, EntityId>,
    events: &mut Vec<AnimationEvent>,
    event_owners: &mut BTreeMap<EventId, EntityId>,
    storage_issues: &mut Vec<AnimationStorageIssue>,
    universal_bindings: &mut BTreeSet<(String, String, String)>,
    max_tick: &mut Tick,
) {
    let target = entity.to_loro_key();
    let fields = animation_fields(engine, entity);
    load_timeline_records(
        entity,
        &fields,
        markers,
        marker_owners,
        events,
        event_owners,
        storage_issues,
        max_tick,
    );
    let mut legacy_metas: BTreeMap<String, StoredTrackMeta> = BTreeMap::new();
    let mut semantic_tracks: BTreeMap<String, BTreeMap<String, FieldValue>> = BTreeMap::new();
    let mut legacy_keys: BTreeMap<(String, String), Keyframe> = BTreeMap::new();
    let mut semantic_keys: BTreeMap<(String, String), BTreeMap<String, FieldValue>> =
        BTreeMap::new();
    collect_animation_records(
        entity,
        fields,
        &mut legacy_metas,
        &mut semantic_tracks,
        &mut legacy_keys,
        &mut semantic_keys,
        storage_issues,
    );

    let mut track_ids: BTreeSet<String> = legacy_metas.keys().cloned().collect();
    track_ids.extend(semantic_tracks.keys().cloned());
    let mut keys_by_track: BTreeMap<String, Vec<Keyframe>> = BTreeMap::new();
    let mut key_ids: BTreeSet<(String, String)> = legacy_keys.keys().cloned().collect();
    key_ids.extend(semantic_keys.keys().cloned());
    for (track_id, key_id) in key_ids {
        let semantic = semantic_keys.get(&(track_id.clone(), key_id.clone()));
        match reconstruct_key(
            &track_id,
            &key_id,
            legacy_keys.get(&(track_id.clone(), key_id.clone())),
            semantic,
        ) {
            Ok(Some(key)) => keys_by_track.entry(track_id).or_default().push(key),
            Ok(None) => {}
            Err(reason) => storage_issues.push(semantic_storage_error(
                entity,
                &track_id,
                "corrupt_animation_key_field",
                &reason,
            )),
        }
    }

    for track_id in track_ids {
        let seed = match reconstruct_track(
            &track_id,
            legacy_metas.get(&track_id),
            semantic_tracks.get(&track_id),
        ) {
            Ok(seed) => seed,
            Err(reason) => {
                storage_issues.push(semantic_storage_error(
                    entity,
                    &track_id,
                    "corrupt_animation_track_field",
                    &reason,
                ));
                continue;
            }
        };
        let mut track_keys = keys_by_track.remove(&track_id).unwrap_or_default();
        track_keys.sort_by(|left, right| left.tick.cmp(&right.tick).then(left.id.cmp(&right.id)));
        // Retain metadata and tombstones for lossless undo, but do not expose an empty authored
        // track to the timeline or let it suppress a playable legacy JointTrack adapter.
        if track_keys.is_empty() {
            continue;
        }
        if let Some(last) = track_keys.last() {
            *max_tick = (*max_tick).max(last.tick);
        }
        // Track identity is canonical to the actual owner and binding. Composition duplication copies
        // storage fields verbatim; canonicalization prevents a copied track from colliding with its source.
        let public_track_id = stable_track_id(&target, &seed.component, &seed.property);
        let track = Track {
            id: TrackId::new(public_track_id.clone()),
            name: seed.name,
            binding: Binding {
                path: PropertyPath::new(target.clone(), seed.component, seed.property),
                value_kind: seed.value_kind,
            },
            interpolation: seed.interpolation,
            keyframes: track_keys,
            enabled: seed.enabled,
        };
        if seed.locked {
            locked_tracks.insert(TrackId::new(public_track_id));
        }
        universal_bindings.insert((
            target.clone(),
            track.binding.path.component.clone(),
            track.binding.path.property.clone(),
        ));
        tracks.push(track);
    }
    for orphan in keys_by_track.keys() {
        storage_issues.push(AnimationStorageIssue {
            severity: "warning",
            code: "orphan_animation_key",
            message: format!("track {orphan} has keys but no metadata"),
            remediation: Some("Restore its track metadata or remove the orphan keys.".into()),
            target: Some(entity),
        });
    }
}

fn append_legacy_joint_track(
    engine: &Engine<FlecsWorld>,
    entity: EntityId,
    universal_bindings: &BTreeSet<(String, String, String)>,
    tracks: &mut Vec<Track>,
    max_tick: &mut Tick,
) {
    let Some((_joint, joint_component)) = joint_of_with_component(engine, entity) else {
        return;
    };
    let target = entity.to_loro_key();
    let binding_key = (
        target.clone(),
        joint_component.to_owned(),
        "value".to_owned(),
    );
    if universal_bindings.contains(&binding_key) {
        return;
    }
    let encoded = engine
        .components_of(entity)
        .get(JOINT_TRACK)
        .and_then(|component| component.get("keys"))
        .and_then(|value| match value {
            FieldValue::Str(value) => Some(value.clone()),
            _ => None,
        });
    let Some(encoded) = encoded else { return };
    let keyframes: Vec<_> = parse_track(&encoded)
        .into_iter()
        .filter_map(|(seconds, value)| {
            let tick = TimeBase::new(DEFAULT_TICKS_PER_SECOND)
                .from_seconds(seconds)
                .ok()?;
            let key_id = format!("legacy-{}", tick.0);
            Some(Keyframe::new(
                key_id.as_str(),
                tick,
                AnimValue::Number(value),
            ))
        })
        .collect();
    if keyframes.is_empty() {
        return;
    }
    if let Some(last) = keyframes.last() {
        *max_tick = (*max_tick).max(last.tick);
    }
    tracks.push(Track {
        id: TrackId::new(format!("legacy-{}", stable_hash(target.as_bytes()))),
        name: "Joint value (legacy mechanism)".into(),
        binding: Binding {
            path: PropertyPath::new(target, joint_component, "value"),
            value_kind: ValueKind::Number,
        },
        interpolation: Interpolation::Linear,
        keyframes,
        enabled: true,
    });
}

fn animation_fields(engine: &Engine<FlecsWorld>, entity: EntityId) -> BTreeMap<String, FieldValue> {
    engine
        .components_of(entity)
        .get(ANIMATION_TRACK)
        .map(|fields| {
            fields
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn set_animation_field(entity: EntityId, field: String, value: FieldValue) -> Op {
    Op::SetField {
        entity,
        component: ANIMATION_TRACK.into(),
        field,
        value,
    }
}

fn track_create_ops(entity: EntityId, track: &Track) -> Vec<Op> {
    let id = track.id.as_str();
    [
        (
            "schema",
            FieldValue::Integer(i64::from(ANIMATION_TRACK_SCHEMA_VERSION)),
        ),
        ("name", FieldValue::Str(track.name.clone())),
        (
            "component",
            FieldValue::Str(track.binding.path.component.clone()),
        ),
        (
            "property",
            FieldValue::Str(track.binding.path.property.clone()),
        ),
        (
            "value_kind",
            FieldValue::Str(value_kind_name(track.binding.value_kind).into()),
        ),
        (
            "interpolation",
            FieldValue::Str(interpolation_name(track.interpolation).into()),
        ),
        ("enabled", FieldValue::Bool(track.enabled)),
        ("locked", FieldValue::Bool(false)),
    ]
    .into_iter()
    .map(|(name, value)| set_animation_field(entity, track_semantic_field(id, name), value))
    .collect()
}

fn key_create_ops(
    entity: EntityId,
    track_id: &str,
    key: &Keyframe,
) -> Result<Vec<Op>, AnimationIntentError> {
    Ok([
        ("active", FieldValue::Bool(true)),
        ("tick", FieldValue::Integer(key.tick.0)),
        ("value", FieldValue::Str(encode(&key.value)?)),
        ("in_tangent", FieldValue::Str(encode(&key.in_tangent)?)),
        ("out_tangent", FieldValue::Str(encode(&key.out_tangent)?)),
    ]
    .into_iter()
    .map(|(name, value)| {
        set_animation_field(
            entity,
            key_semantic_field(track_id, key.id.as_str(), name),
            value,
        )
    })
    .collect())
}

#[allow(clippy::too_many_arguments)]
fn collect_animation_records(
    entity: EntityId,
    fields: BTreeMap<String, FieldValue>,
    legacy_metas: &mut BTreeMap<String, StoredTrackMeta>,
    semantic_tracks: &mut BTreeMap<String, BTreeMap<String, FieldValue>>,
    legacy_keys: &mut BTreeMap<(String, String), Keyframe>,
    semantic_keys: &mut BTreeMap<(String, String), BTreeMap<String, FieldValue>>,
    issues: &mut Vec<AnimationStorageIssue>,
) {
    for (field, value) in fields {
        if let Some(rest) = field.strip_prefix(TRACK_FIELD_PREFIX) {
            if let Some((track_id, semantic_name)) = rest.split_once("::") {
                semantic_tracks
                    .entry(track_id.to_owned())
                    .or_default()
                    .insert(semantic_name.to_owned(), value);
            } else if let FieldValue::Str(payload) = value {
                match decode::<StoredTrackMeta>(&payload) {
                    Ok(meta) if meta.schema_version == LEGACY_ANIMATION_TRACK_SCHEMA_VERSION => {
                        if meta.id.as_str() == rest {
                            legacy_metas.insert(rest.to_owned(), meta);
                        } else {
                            issues.push(semantic_storage_error(
                                entity,
                                rest,
                                "track_identity_mismatch",
                                &format!("legacy payload contains identity {}", meta.id),
                            ));
                        }
                    }
                    Ok(meta) => issues.push(AnimationStorageIssue {
                        severity: "error",
                        code: "unsupported_track_schema",
                        message: format!("track {rest} uses schema {}", meta.schema_version),
                        remediation: Some("Migrate the track before playback.".into()),
                        target: Some(entity),
                    }),
                    Err(error) => issues.push(storage_error(entity, rest, &error)),
                }
            } else {
                issues.push(semantic_storage_error(
                    entity,
                    rest,
                    "corrupt_animation_track",
                    "legacy track metadata is not a string payload",
                ));
            }
        } else if let Some(rest) = field.strip_prefix(KEY_FIELD_PREFIX) {
            let Some((track_id, tail)) = rest.split_once("::") else {
                issues.push(semantic_storage_error(
                    entity,
                    rest,
                    "corrupt_animation_key_field",
                    "malformed key field name",
                ));
                continue;
            };
            if let Some((key_id, semantic_name)) = tail.split_once("::") {
                semantic_keys
                    .entry((track_id.to_owned(), key_id.to_owned()))
                    .or_default()
                    .insert(semantic_name.to_owned(), value);
            } else if let FieldValue::Str(payload) = value {
                match decode::<Keyframe>(&payload) {
                    Ok(key) if key.id.as_str() == tail => {
                        legacy_keys.insert((track_id.to_owned(), tail.to_owned()), key);
                    }
                    Ok(key) => issues.push(semantic_storage_error(
                        entity,
                        track_id,
                        "key_identity_mismatch",
                        &format!("key field {tail} contains identity {}", key.id),
                    )),
                    Err(error) => issues.push(storage_error(entity, track_id, &error)),
                }
            } else {
                issues.push(semantic_storage_error(
                    entity,
                    track_id,
                    "corrupt_animation_key_field",
                    &format!("legacy key {tail} is not a string payload"),
                ));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn load_timeline_records(
    owner: EntityId,
    fields: &BTreeMap<String, FieldValue>,
    markers: &mut Vec<Marker>,
    marker_owners: &mut BTreeMap<MarkerId, EntityId>,
    events: &mut Vec<AnimationEvent>,
    event_owners: &mut BTreeMap<EventId, EntityId>,
    issues: &mut Vec<AnimationStorageIssue>,
    max_tick: &mut Tick,
) {
    let marker_records = collect_named_records(fields, MARKER_FIELD_PREFIX);
    for (storage_id, record) in marker_records {
        let active = record
            .get("active")
            .map_or(Ok(true), |value| bool_field(value, "marker.active"));
        let parsed = (|| -> Result<Option<Marker>, String> {
            if !active? {
                return Ok(None);
            }
            let tick = Tick(integer_field(
                required_field(&record, "tick", "marker")?,
                "marker.tick",
            )?);
            if tick.0 < 0 {
                return Err("marker tick must be non-negative".into());
            }
            let color = record
                .get("color")
                .map_or(Ok(None), |value| decode_field(value, "marker.color"))?;
            Ok(Some(Marker {
                id: MarkerId::new(canonical_record_id(owner, "marker", &storage_id)),
                name: string_field(required_field(&record, "name", "marker")?, "marker.name")?
                    .to_owned(),
                tick,
                color,
            }))
        })();
        match parsed {
            Ok(Some(marker)) => {
                *max_tick = (*max_tick).max(marker.tick);
                marker_owners.insert(marker.id.clone(), owner);
                markers.push(marker);
            }
            Ok(None) => {}
            Err(reason) => issues.push(timeline_storage_error(
                owner,
                "corrupt_animation_marker",
                &storage_id,
                &reason,
            )),
        }
    }

    let event_records = collect_named_records(fields, EVENT_FIELD_PREFIX);
    for (storage_id, record) in event_records {
        let active = record
            .get("active")
            .map_or(Ok(true), |value| bool_field(value, "event.active"));
        let parsed = (|| -> Result<Option<AnimationEvent>, String> {
            if !active? {
                return Ok(None);
            }
            let tick = Tick(integer_field(
                required_field(&record, "tick", "event")?,
                "event.tick",
            )?);
            if tick.0 < 0 {
                return Err("event tick must be non-negative".into());
            }
            let payload = record
                .get("payload")
                .map_or(Ok(None), |value| decode_field(value, "event.payload"))?;
            Ok(Some(AnimationEvent {
                id: EventId::new(canonical_record_id(owner, "event", &storage_id)),
                name: string_field(required_field(&record, "name", "event")?, "event.name")?
                    .to_owned(),
                tick,
                payload,
            }))
        })();
        match parsed {
            Ok(Some(event)) => {
                *max_tick = (*max_tick).max(event.tick);
                event_owners.insert(event.id.clone(), owner);
                events.push(event);
            }
            Ok(None) => {}
            Err(reason) => issues.push(timeline_storage_error(
                owner,
                "corrupt_animation_event",
                &storage_id,
                &reason,
            )),
        }
    }
}

fn collect_named_records(
    fields: &BTreeMap<String, FieldValue>,
    prefix: &str,
) -> BTreeMap<String, BTreeMap<String, FieldValue>> {
    let mut records: BTreeMap<String, BTreeMap<String, FieldValue>> = BTreeMap::new();
    for (field, value) in fields {
        let Some(rest) = field.strip_prefix(prefix) else {
            continue;
        };
        let Some((id, name)) = rest.split_once("::") else {
            continue;
        };
        records
            .entry(id.to_owned())
            .or_default()
            .insert(name.to_owned(), value.clone());
    }
    records
}

fn reconstruct_track(
    track_id: &str,
    legacy: Option<&StoredTrackMeta>,
    semantic: Option<&BTreeMap<String, FieldValue>>,
) -> Result<TrackSeed, String> {
    let mut seed = legacy.map(TrackSeed::from_legacy);
    let Some(fields) = semantic else {
        return seed.ok_or_else(|| "track has no metadata".into());
    };
    if let Some(schema) = fields.get("schema") {
        let version = integer_field(schema, "schema")?;
        if version != i64::from(ANIMATION_TRACK_SCHEMA_VERSION) {
            return Err(format!("unsupported semantic schema {version}"));
        }
    } else if seed.is_none() {
        return Err("incomplete semantic track: missing schema".into());
    }
    if seed.is_none() {
        seed = Some(TrackSeed {
            name: string_field(required_field(fields, "name", "track")?, "name")?.into(),
            component: string_field(required_field(fields, "component", "track")?, "component")?
                .into(),
            property: string_field(required_field(fields, "property", "track")?, "property")?
                .into(),
            value_kind: parse_value_kind(string_field(
                required_field(fields, "value_kind", "track")?,
                "value_kind",
            )?)?,
            interpolation: parse_interpolation(string_field(
                required_field(fields, "interpolation", "track")?,
                "interpolation",
            )?)?,
            enabled: bool_field(required_field(fields, "enabled", "track")?, "enabled")?,
            locked: fields
                .get("locked")
                .map_or(Ok(false), |value| bool_field(value, "locked"))?,
        });
    }
    let mut seed = seed.expect("seed created above");
    if let Some(value) = fields.get("name") {
        seed.name = string_field(value, "name")?.into();
    }
    if let Some(value) = fields.get("component") {
        seed.component = string_field(value, "component")?.into();
    }
    if let Some(value) = fields.get("property") {
        seed.property = string_field(value, "property")?.into();
    }
    if let Some(value) = fields.get("value_kind") {
        seed.value_kind = parse_value_kind(string_field(value, "value_kind")?)?;
    }
    if let Some(value) = fields.get("interpolation") {
        seed.interpolation = parse_interpolation(string_field(value, "interpolation")?)?;
    }
    if let Some(value) = fields.get("enabled") {
        seed.enabled = bool_field(value, "enabled")?;
    }
    if let Some(value) = fields.get("locked") {
        seed.locked = bool_field(value, "locked")?;
    }
    if seed.name.trim().is_empty()
        || seed.component.trim().is_empty()
        || seed.property.trim().is_empty()
    {
        return Err(format!(
            "track {track_id} contains an empty name or binding segment"
        ));
    }
    Ok(seed)
}

fn reconstruct_key(
    track_id: &str,
    key_id: &str,
    legacy: Option<&Keyframe>,
    semantic: Option<&BTreeMap<String, FieldValue>>,
) -> Result<Option<Keyframe>, String> {
    let Some(fields) = semantic else {
        return Ok(legacy.cloned());
    };
    let active = match fields.get("active") {
        Some(value) => bool_field(value, "active")?,
        None if legacy.is_some() => true,
        None => {
            return Err(format!(
                "key {key_id} on track {track_id} is missing active"
            ))
        }
    };
    if !active {
        return Ok(None);
    }
    let tick = match fields.get("tick") {
        Some(value) => Tick(integer_field(value, "tick")?),
        None if legacy.is_some() => legacy.expect("checked").tick,
        None => return Err(format!("key {key_id} on track {track_id} is missing tick")),
    };
    if tick.0 < 0 {
        return Err(format!("key {key_id} has negative tick {}", tick.0));
    }
    let value = match fields.get("value") {
        Some(value) => decode_field(value, "value")?,
        None if legacy.is_some() => legacy.expect("checked").value.clone(),
        None => return Err(format!("key {key_id} on track {track_id} is missing value")),
    };
    let in_tangent = match fields.get("in_tangent") {
        Some(value) => decode_field(value, "in_tangent")?,
        None if legacy.is_some() => legacy.expect("checked").in_tangent.clone(),
        None => {
            return Err(format!(
                "key {key_id} on track {track_id} is missing in_tangent"
            ))
        }
    };
    let out_tangent = match fields.get("out_tangent") {
        Some(value) => decode_field(value, "out_tangent")?,
        None if legacy.is_some() => legacy.expect("checked").out_tangent.clone(),
        None => {
            return Err(format!(
                "key {key_id} on track {track_id} is missing out_tangent"
            ))
        }
    };
    Ok(Some(Keyframe {
        id: key_id.into(),
        tick,
        value,
        in_tangent,
        out_tangent,
    }))
}

fn required_field<'a>(
    fields: &'a BTreeMap<String, FieldValue>,
    name: &str,
    record: &str,
) -> Result<&'a FieldValue, String> {
    fields
        .get(name)
        .ok_or_else(|| format!("incomplete semantic {record}: missing {name}"))
}

fn string_field<'a>(value: &'a FieldValue, name: &str) -> Result<&'a str, String> {
    match value {
        FieldValue::Str(value) => Ok(value),
        _ => Err(format!("semantic field {name} must be a string")),
    }
}

fn bool_field(value: &FieldValue, name: &str) -> Result<bool, String> {
    match value {
        FieldValue::Bool(value) => Ok(*value),
        _ => Err(format!("semantic field {name} must be a boolean")),
    }
}

fn integer_field(value: &FieldValue, name: &str) -> Result<i64, String> {
    match value {
        FieldValue::Integer(value) => Ok(*value),
        _ => Err(format!("semantic field {name} must be an integer")),
    }
}

fn decode_field<T: DeserializeOwned>(value: &FieldValue, name: &str) -> Result<T, String> {
    let payload = string_field(value, name)?;
    decode(payload).map_err(|error| format!("semantic field {name}: {error}"))
}

const fn interpolation_name(value: Interpolation) -> &'static str {
    match value {
        Interpolation::Step => "step",
        Interpolation::Linear => "linear",
        Interpolation::Cubic => "cubic",
    }
}

fn parse_interpolation(value: &str) -> Result<Interpolation, String> {
    match value {
        "step" => Ok(Interpolation::Step),
        "linear" => Ok(Interpolation::Linear),
        "cubic" => Ok(Interpolation::Cubic),
        _ => Err(format!("unknown interpolation {value:?}")),
    }
}

const fn value_kind_name(value: ValueKind) -> &'static str {
    match value {
        ValueKind::Number => "number",
        ValueKind::Integer => "integer",
        ValueKind::Boolean => "boolean",
        ValueKind::String => "string",
        ValueKind::Vec3 => "vec3",
        ValueKind::Vec4 => "vec4",
        ValueKind::Quaternion => "quaternion",
        ValueKind::Weights => "weights",
    }
}

fn parse_value_kind(value: &str) -> Result<ValueKind, String> {
    match value {
        "number" => Ok(ValueKind::Number),
        "integer" => Ok(ValueKind::Integer),
        "boolean" => Ok(ValueKind::Boolean),
        "string" => Ok(ValueKind::String),
        "vec3" => Ok(ValueKind::Vec3),
        "vec4" => Ok(ValueKind::Vec4),
        "quaternion" => Ok(ValueKind::Quaternion),
        "weights" => Ok(ValueKind::Weights),
        _ => Err(format!("unknown value kind {value:?}")),
    }
}

fn protection_reason(
    component: &str,
    property: &str,
    value: &FieldValue,
    kinematic_component: Option<&'static str>,
    binding: &AnimationBindingContext,
) -> Option<String> {
    if component == ANIMATION_TRACK {
        return Some(
            "Animation storage is edited through the timeline, not animated recursively.".into(),
        );
    }
    if component == "__meta__" || component == "CadPart" || component == "ReimportIdentity" {
        return Some(
            "Identity, provenance and re-import fields remain stable authored data.".into(),
        );
    }
    if component == "MeshRenderer" {
        return Some(
            "Mesh and material asset references cannot change inside a property track.".into(),
        );
    }
    if matches!(component, "RigidBody" | "Collider" | "PhysicsJoint") {
        return Some("Physics structure has explicit runtime authority; animate Transform or a kinematic-joint value.".into());
    }
    if component == "Joint" && kinematic_component != Some("Joint") {
        return Some("This is a physics joint schema, not a kinematic animation control.".into());
    }
    if matches!(component, "KinematicJoint" | "Joint") && property != "value" {
        return Some(
            "A joint's axis, pivot and limits define its rig; animate only its value.".into(),
        );
    }
    let lower = property.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "id" | "source"
            | "handle"
            | "parent"
            | "bodya"
            | "bodyb"
            | "controller"
            | "reference"
            | "hash"
    ) || lower.ends_with("_id")
        || lower.ends_with("_handle")
        || lower.ends_with("_hash")
    {
        return Some(
            "References and source identities cannot be sampled as animation values.".into(),
        );
    }
    if matches!(value, FieldValue::Number(number) if !number.is_finite()) {
        return Some("The current value is not finite; repair it before keying.".into());
    }
    if binding
        .value_kind
        .is_some_and(|expected| expected != value_kind(value))
    {
        return Some("The live field type does not match its registered animation binding; repair the component schema before keying.".into());
    }
    let target_valid = matches!(
        binding.readiness,
        AnimationReadiness::Ready | AnimationReadiness::PreviewOnly
    ) || (binding.readiness == AnimationReadiness::RequiresValidation
        && matches!(component, "KinematicJoint" | "Joint")
        && property == "value"
        && kinematic_component == Some(component));
    if target_valid && binding.runtime_sink.is_some() {
        return None;
    }
    binding.diagnostics.first().map_or_else(
        || Some("This typed property has no verified animation consumer yet; keying is disabled so playback cannot fail silently.".into()),
        |diagnostic| Some(format!("{} {}", diagnostic.message, diagnostic.remediation)),
    )
}

fn value_kind(value: &FieldValue) -> ValueKind {
    match value {
        FieldValue::Number(_) => ValueKind::Number,
        FieldValue::Integer(_) => ValueKind::Integer,
        FieldValue::Bool(_) => ValueKind::Boolean,
        FieldValue::Str(_) => ValueKind::String,
    }
}

fn anim_value(value: &FieldValue) -> AnimValue {
    match value {
        FieldValue::Number(value) => AnimValue::Number(*value),
        FieldValue::Integer(value) => AnimValue::Integer(*value),
        FieldValue::Bool(value) => AnimValue::Boolean(*value),
        FieldValue::Str(value) => AnimValue::String(value.clone()),
    }
}

fn display_token(value: &str) -> String {
    value.replace('_', " ")
}

fn stable_track_id(target: &str, component: &str, property: &str) -> String {
    let identity = format!("{target}\0{component}\0{property}");
    format!("track-{:016x}", stable_hash(identity.as_bytes()))
}

fn allocate_element_id(engine: &mut Engine<FlecsWorld>, kind: &str) -> String {
    format!("{kind}-{}", engine.alloc_entity_id().to_loro_key())
}

fn generated_element_peer(id: &str, kind: &str) -> Option<u64> {
    let generated = id.strip_prefix(kind)?.strip_prefix('-')?;
    EntityId::from_loro_key(generated).map(|id| id.peer)
}

fn canonical_record_id(owner: EntityId, kind: &str, storage_id: &str) -> String {
    let identity = format!("{}\0{kind}\0{storage_id}", owner.to_loro_key());
    format!("{kind}-{:016x}", stable_hash(identity.as_bytes()))
}

fn resolve_storage_track_id(
    engine: &Engine<FlecsWorld>,
    entity: EntityId,
    public_track_id: &str,
) -> Option<String> {
    let fields = animation_fields(engine, entity);
    let mut legacy_metas = BTreeMap::new();
    let mut semantic_tracks = BTreeMap::new();
    let mut ignored_legacy_keys = BTreeMap::new();
    let mut ignored_semantic_keys = BTreeMap::new();
    let mut ignored_issues = Vec::new();
    collect_animation_records(
        entity,
        fields,
        &mut legacy_metas,
        &mut semantic_tracks,
        &mut ignored_legacy_keys,
        &mut ignored_semantic_keys,
        &mut ignored_issues,
    );
    let mut ids: BTreeSet<String> = legacy_metas.keys().cloned().collect();
    ids.extend(semantic_tracks.keys().cloned());
    let target = entity.to_loro_key();
    ids.into_iter().find(|storage_id| {
        reconstruct_track(
            storage_id,
            legacy_metas.get(storage_id),
            semantic_tracks.get(storage_id),
        )
        .is_ok_and(|seed| {
            stable_track_id(&target, &seed.component, &seed.property) == public_track_id
        })
    })
}

fn resolve_record_id(
    engine: &Engine<FlecsWorld>,
    owner: EntityId,
    prefix: &str,
    kind: &str,
    public_id: &str,
) -> Option<String> {
    collect_named_records(&animation_fields(engine, owner), prefix)
        .into_keys()
        .find(|storage_id| canonical_record_id(owner, kind, storage_id) == public_id)
}

fn record_exists(
    engine: &Engine<FlecsWorld>,
    owner: EntityId,
    prefix: &str,
    storage_id: &str,
) -> bool {
    let expected = format!("{prefix}{storage_id}::");
    animation_fields(engine, owner)
        .keys()
        .any(|field| field.starts_with(&expected))
}

fn non_empty_name(name: &str, fallback: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn revision_hash_frame(hash: &mut u64, bytes: &[u8]) {
    for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn track_field(track_id: &str) -> String {
    format!("{TRACK_FIELD_PREFIX}{track_id}")
}

fn track_semantic_field(track_id: &str, name: &str) -> String {
    format!("{}::{name}", track_field(track_id))
}

fn key_field(track_id: &str, key_id: &str) -> String {
    format!("{KEY_FIELD_PREFIX}{track_id}::{key_id}")
}

fn key_semantic_field(track_id: &str, key_id: &str, name: &str) -> String {
    format!("{}::{name}", key_field(track_id, key_id))
}

fn marker_semantic_field(marker_id: &str, name: &str) -> String {
    format!("{MARKER_FIELD_PREFIX}{marker_id}::{name}")
}

fn event_semantic_field(event_id: &str, name: &str) -> String {
    format!("{EVENT_FIELD_PREFIX}{event_id}::{name}")
}

fn encode<T: Serialize>(value: &T) -> Result<String, AnimationIntentError> {
    let bytes = bincode::serialize(value)
        .map_err(|error| AnimationIntentError::CorruptTrack(error.to_string()))?;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    Ok(out)
}

fn decode<T: DeserializeOwned>(payload: &str) -> Result<T, AnimationIntentError> {
    if !payload.len().is_multiple_of(2) {
        return Err(AnimationIntentError::CorruptTrack(
            "binary payload has an odd hex length".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(payload.len() / 2);
    for pair in payload.as_bytes().as_chunks::<2>().0 {
        let text = std::str::from_utf8(pair)
            .map_err(|error| AnimationIntentError::CorruptTrack(error.to_string()))?;
        bytes.push(
            u8::from_str_radix(text, 16)
                .map_err(|error| AnimationIntentError::CorruptTrack(error.to_string()))?,
        );
    }
    bincode::deserialize(&bytes)
        .map_err(|error| AnimationIntentError::CorruptTrack(error.to_string()))
}

fn storage_error(
    target: EntityId,
    track_id: &str,
    error: &AnimationIntentError,
) -> AnimationStorageIssue {
    AnimationStorageIssue {
        severity: "error",
        code: "corrupt_animation_track",
        message: format!("track {track_id}: {error}"),
        remediation: Some("Restore this track from history or remove its damaged fields.".into()),
        target: Some(target),
    }
}

fn semantic_storage_error(
    target: EntityId,
    track_id: &str,
    code: &'static str,
    reason: &str,
) -> AnimationStorageIssue {
    AnimationStorageIssue {
        severity: "error",
        code,
        message: format!("track {track_id}: {reason}"),
        remediation: Some(
            "Restore the named semantic field from history or remove the incomplete record.".into(),
        ),
        target: Some(target),
    }
}

fn timeline_storage_error(
    target: EntityId,
    code: &'static str,
    record_id: &str,
    reason: &str,
) -> AnimationStorageIssue {
    AnimationStorageIssue {
        severity: "error",
        code,
        message: format!("timeline record {record_id}: {reason}"),
        remediation: Some(
            "Restore the named marker/event fields from history or remove the incomplete record."
                .into(),
        ),
        target: Some(target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(engine: &mut Engine<FlecsWorld>) -> EntityId {
        let id = engine.alloc_entity_id();
        engine
            .commit(
                "target",
                vec![
                    Op::CreateEntity { id, parent: None },
                    Op::SetField {
                        entity: id,
                        component: "Transform".into(),
                        field: "x".into(),
                        value: FieldValue::Number(2.0),
                    },
                    Op::SetField {
                        entity: id,
                        component: "MeshRenderer".into(),
                        field: "mesh".into(),
                        value: FieldValue::Str("mtkasset:source".into()),
                    },
                ],
            )
            .expect("create target");
        id
    }

    #[test]
    fn one_key_is_one_granular_field_and_undo_removes_the_edit() {
        let mut engine = Engine::new(FlecsWorld::new(), 41);
        let id = target(&mut engine);
        let first = key_property(
            &mut engine,
            id,
            "Transform",
            "x",
            Tick(0),
            Interpolation::Linear,
        )
        .expect("key property");
        let fields = animation_fields(&engine, id);
        assert_eq!(
            fields.get(&track_semantic_field(&first.track_id, "schema")),
            Some(&FieldValue::Integer(i64::from(
                ANIMATION_TRACK_SCHEMA_VERSION
            )))
        );
        assert_eq!(
            fields.get(&key_semantic_field(
                &first.track_id,
                &first.key_id,
                "active"
            )),
            Some(&FieldValue::Bool(true))
        );
        assert!(!fields.contains_key(&track_field(&first.track_id)));
        assert!(!fields.contains_key(&key_field(&first.track_id, &first.key_id)));
        assert_eq!(load_animation_document(&engine).sequence.tracks.len(), 1);
        assert!(engine.undo(), "undo key");
        assert!(animation_fields(&engine, id).is_empty());
    }

    #[test]
    fn same_tick_rekey_replaces_locally_and_samples_deterministically() {
        let mut engine = Engine::new(FlecsWorld::new(), 43);
        let id = target(&mut engine);
        key_property(
            &mut engine,
            id,
            "Transform",
            "x",
            Tick(10),
            Interpolation::Linear,
        )
        .expect("first key");
        engine
            .commit(
                "move",
                vec![Op::SetField {
                    entity: id,
                    component: "Transform".into(),
                    field: "x".into(),
                    value: FieldValue::Number(9.0),
                }],
            )
            .expect("change value");
        let receipt = key_property(
            &mut engine,
            id,
            "Transform",
            "x",
            Tick(10),
            Interpolation::Linear,
        )
        .expect("replace key");
        assert_eq!(receipt.replaced_keys, 1);
        let document = load_animation_document(&engine);
        assert_eq!(document.sequence.tracks[0].keyframes.len(), 1);
        let compiled = document.compile().expect("compile");
        let first = compiled.evaluate(Tick(10));
        let second = compiled.evaluate(Tick(10));
        assert_eq!(first, second);
        assert_eq!(first.bindings[0].value, AnimValue::Number(9.0));
    }

    #[test]
    fn protected_asset_references_are_visible_but_cannot_be_keyed() {
        let mut engine = Engine::new(FlecsWorld::new(), 47);
        let id = target(&mut engine);
        let mesh = property_catalog(&engine, id)
            .into_iter()
            .find(|fact| fact.component == "MeshRenderer" && fact.property == "mesh")
            .expect("mesh fact");
        assert!(!mesh.animatable);
        assert!(mesh.reason.is_some());
        assert!(matches!(
            key_property(
                &mut engine,
                id,
                "MeshRenderer",
                "mesh",
                Tick(0),
                Interpolation::Step,
            ),
            Err(AnimationIntentError::ProtectedProperty(_))
        ));

        engine
            .commit(
                "typed and kinematic properties",
                vec![
                    Op::SetField {
                        entity: id,
                        component: "CustomMaterial".into(),
                        field: "roughness".into(),
                        value: FieldValue::Number(0.4),
                    },
                    Op::SetField {
                        entity: id,
                        component: "KinematicJoint".into(),
                        field: "ax".into(),
                        value: FieldValue::Number(1.0),
                    },
                    Op::SetField {
                        entity: id,
                        component: "KinematicJoint".into(),
                        field: "ay".into(),
                        value: FieldValue::Number(0.0),
                    },
                    Op::SetField {
                        entity: id,
                        component: "KinematicJoint".into(),
                        field: "az".into(),
                        value: FieldValue::Number(0.0),
                    },
                    Op::SetField {
                        entity: id,
                        component: "KinematicJoint".into(),
                        field: "px".into(),
                        value: FieldValue::Number(0.0),
                    },
                    Op::SetField {
                        entity: id,
                        component: "KinematicJoint".into(),
                        field: "py".into(),
                        value: FieldValue::Number(0.0),
                    },
                    Op::SetField {
                        entity: id,
                        component: "KinematicJoint".into(),
                        field: "pz".into(),
                        value: FieldValue::Number(0.0),
                    },
                    Op::SetField {
                        entity: id,
                        component: "KinematicJoint".into(),
                        field: "value".into(),
                        value: FieldValue::Number(0.0),
                    },
                ],
            )
            .expect("add typed properties");
        let catalog = property_catalog(&engine, id);
        let unsupported = catalog
            .iter()
            .find(|fact| fact.component == "CustomMaterial" && fact.property == "roughness")
            .expect("unsupported typed field remains visible");
        assert!(!unsupported.animatable);
        assert!(unsupported
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("no registered animation binding")));
        assert!(
            catalog
                .iter()
                .find(|fact| fact.component == "Transform" && fact.property == "x")
                .expect("transform field")
                .animatable
        );
        assert!(
            catalog
                .iter()
                .find(|fact| fact.component == "KinematicJoint" && fact.property == "value")
                .expect("validated kinematic value")
                .animatable
        );
    }

    #[test]
    fn concurrent_semantic_track_fields_merge_without_clobbering() {
        let mut base = Engine::new(FlecsWorld::new(), 48);
        let id = target(&mut base);
        let receipt = key_property(
            &mut base,
            id,
            "Transform",
            "x",
            Tick(0),
            Interpolation::Linear,
        )
        .expect("seed track");
        let snapshot = base.export_updates();
        let mut left = Engine::new(FlecsWorld::new(), 49);
        let mut right = Engine::new(FlecsWorld::new(), 50);
        left.merge(&snapshot).expect("fork left");
        right.merge(&snapshot).expect("fork right");

        set_track_interpolation(&mut left, id, &receipt.track_id, Interpolation::Cubic)
            .expect("edit interpolation");
        right
            .commit(
                "rename track",
                vec![set_animation_field(
                    id,
                    track_semantic_field(&receipt.track_id, "name"),
                    FieldValue::Str("Hero entrance".into()),
                )],
            )
            .expect("edit name");
        let left_updates = left.export_updates();
        let right_updates = right.export_updates();
        left.merge(&right_updates).expect("merge name");
        right.merge(&left_updates).expect("merge interpolation");

        assert_eq!(left.canonical_state(), right.canonical_state());
        let track = &load_animation_document(&left).sequence.tracks[0];
        assert_eq!(track.name, "Hero entrance");
        assert_eq!(track.interpolation, Interpolation::Cubic);
    }

    #[test]
    fn deletion_tombstones_a_key_and_undo_restores_it() {
        let mut engine = Engine::new(FlecsWorld::new(), 54);
        let id = target(&mut engine);
        let receipt = key_property(
            &mut engine,
            id,
            "Transform",
            "x",
            Tick(20),
            Interpolation::Linear,
        )
        .expect("key property");
        delete_key(&mut engine, id, &receipt.track_id, &receipt.key_id).expect("delete key");
        assert!(load_animation_document(&engine).sequence.tracks.is_empty());
        assert_eq!(
            animation_fields(&engine, id).get(&key_semantic_field(
                &receipt.track_id,
                &receipt.key_id,
                "active"
            )),
            Some(&FieldValue::Bool(false))
        );
        assert!(engine.undo(), "undo tombstone");
        assert_eq!(load_animation_document(&engine).sequence.tracks.len(), 1);
    }

    #[test]
    fn legacy_v1_blobs_remain_readable_and_accept_semantic_overrides() {
        let mut engine = Engine::new(FlecsWorld::new(), 55);
        let id = target(&mut engine);
        let target = id.to_loro_key();
        let track_id = stable_track_id(&target, "Transform", "x");
        let track = Track {
            id: TrackId::new(track_id.clone()),
            name: "Legacy X".into(),
            binding: Binding {
                path: PropertyPath::new(target, "Transform", "x"),
                value_kind: ValueKind::Number,
            },
            interpolation: Interpolation::Linear,
            keyframes: Vec::new(),
            enabled: true,
        };
        let key = Keyframe::new("legacy-key", Tick(7), AnimValue::Number(3.0));
        engine
            .commit(
                "legacy animation",
                vec![
                    set_animation_field(
                        id,
                        track_field(&track_id),
                        FieldValue::Str(
                            encode(&StoredTrackMeta::from_track(&track)).expect("meta"),
                        ),
                    ),
                    set_animation_field(
                        id,
                        key_field(&track_id, key.id.as_str()),
                        FieldValue::Str(encode(&key).expect("key")),
                    ),
                ],
            )
            .expect("store legacy blobs");
        assert_eq!(
            load_animation_document(&engine).sequence.tracks[0].keyframes,
            vec![key]
        );
        set_track_interpolation(&mut engine, id, &track_id, Interpolation::Cubic)
            .expect("semantic override");
        let loaded = load_animation_document(&engine);
        assert_eq!(
            loaded.sequence.tracks[0].interpolation,
            Interpolation::Cubic
        );
        assert!(loaded.storage_issues.is_empty());
    }

    #[test]
    fn concurrent_same_tick_keys_survive_merge_and_converge_by_stable_identity() {
        let mut base = Engine::new(FlecsWorld::new(), 51);
        let id = target(&mut base);
        let snapshot = base.export_updates();
        let mut left = Engine::new(FlecsWorld::new(), 52);
        let mut right = Engine::new(FlecsWorld::new(), 53);
        left.merge(&snapshot).expect("fork left");
        right.merge(&snapshot).expect("fork right");

        for (peer, value) in [(&mut left, 4.0), (&mut right, 8.0)] {
            peer.commit(
                "pose",
                vec![Op::SetField {
                    entity: id,
                    component: "Transform".into(),
                    field: "x".into(),
                    value: FieldValue::Number(value),
                }],
            )
            .expect("author pose");
            key_property(
                peer,
                id,
                "Transform",
                "x",
                Tick(30_000),
                Interpolation::Linear,
            )
            .expect("author concurrent key");
        }

        let left_delta = left.export_updates();
        let right_delta = right.export_updates();
        left.merge(&right_delta).expect("merge right into left");
        right.merge(&left_delta).expect("merge left into right");

        assert_eq!(left.canonical_state(), right.canonical_state());
        let left_document = load_animation_document(&left);
        let right_document = load_animation_document(&right);
        assert_eq!(left_document.sequence, right_document.sequence);
        assert_eq!(left_document.sequence.tracks[0].keyframes.len(), 2);
        let left_sample = left_document
            .compile()
            .expect("compile left")
            .evaluate(Tick(30_000));
        let right_sample = right_document
            .compile()
            .expect("compile right")
            .evaluate(Tick(30_000));
        assert_eq!(left_sample, right_sample);
    }

    #[test]
    fn moving_a_key_retains_its_generated_identity_and_is_one_undo_step() {
        let mut engine = Engine::new(FlecsWorld::new(), 61);
        let id = target(&mut engine);
        let receipt = key_property(
            &mut engine,
            id,
            "Transform",
            "x",
            Tick(5),
            Interpolation::Cubic,
        )
        .expect("key property");
        update_keys(
            &mut engine,
            id,
            &receipt.track_id,
            &[AnimationKeyUpdate {
                key_id: receipt.key_id.clone(),
                tick: Tick(25),
                value: Some(AnimValue::Number(7.0)),
                in_tangent: Some(Some(AnimValue::Number(0.25))),
                out_tangent: Some(Some(AnimValue::Number(0.5))),
            }],
        )
        .expect("move key");
        let key = &load_animation_document(&engine).sequence.tracks[0].keyframes[0];
        assert_eq!(key.id.as_str(), receipt.key_id);
        assert_eq!(key.tick, Tick(25));
        assert_eq!(key.value, AnimValue::Number(7.0));
        assert_eq!(key.in_tangent, Some(AnimValue::Number(0.25)));
        assert!(engine.undo(), "undo the batch edit");
        let restored = &load_animation_document(&engine).sequence.tracks[0].keyframes[0];
        assert_eq!(restored.id.as_str(), receipt.key_id);
        assert_eq!(restored.tick, Tick(5));
    }

    #[test]
    fn copied_animation_storage_is_canonicalized_to_independent_track_identity() {
        let mut engine = Engine::new(FlecsWorld::new(), 62);
        let source = target(&mut engine);
        let receipt = key_property(
            &mut engine,
            source,
            "Transform",
            "x",
            Tick(10),
            Interpolation::Linear,
        )
        .expect("source key");
        let clone = engine.alloc_entity_id();
        let mut ops = vec![Op::CreateEntity {
            id: clone,
            parent: None,
        }];
        for (component, fields) in engine.components_of(source) {
            for (field, value) in fields {
                ops.push(Op::SetField {
                    entity: clone,
                    component: component.clone(),
                    field,
                    value,
                });
            }
        }
        engine.commit("raw copy", ops).expect("copy fields");

        let document = load_animation_document(&engine);
        assert_eq!(document.sequence.tracks.len(), 2);
        assert_ne!(
            document.sequence.tracks[0].id,
            document.sequence.tracks[1].id
        );
        document
            .compile()
            .expect("copied tracks compile independently");
        let clone_track = document
            .sequence
            .tracks
            .iter()
            .find(|track| track.binding.path.target == clone.to_loro_key())
            .expect("clone track")
            .id
            .as_str()
            .to_owned();
        update_keys(
            &mut engine,
            clone,
            &clone_track,
            &[AnimationKeyUpdate {
                key_id: receipt.key_id,
                tick: Tick(40),
                value: None,
                in_tangent: None,
                out_tangent: None,
            }],
        )
        .expect("edit clone through canonical id");
        let document = load_animation_document(&engine);
        let source_tick = document
            .sequence
            .tracks
            .iter()
            .find(|track| track.binding.path.target == source.to_loro_key())
            .unwrap()
            .keyframes[0]
            .tick;
        let clone_tick = document
            .sequence
            .tracks
            .iter()
            .find(|track| track.binding.path.target == clone.to_loro_key())
            .unwrap()
            .keyframes[0]
            .tick;
        assert_eq!(source_tick, Tick(10));
        assert_eq!(clone_tick, Tick(40));
    }

    #[test]
    fn marker_event_mute_and_lock_are_durable_and_undoable() {
        let mut engine = Engine::new(FlecsWorld::new(), 63);
        let id = target(&mut engine);
        let receipt = key_property(
            &mut engine,
            id,
            "Transform",
            "x",
            Tick(0),
            Interpolation::Linear,
        )
        .expect("key");
        let marker = add_marker(
            &mut engine,
            id,
            "Impact",
            Tick(12),
            Some([255, 120, 32, 255]),
        )
        .expect("marker");
        let event = add_event(
            &mut engine,
            id,
            "Footstep",
            Tick(15),
            Some(AnimValue::String("stone".into())),
        )
        .expect("event");
        set_track_enabled(&mut engine, id, &receipt.track_id, false).expect("mute");
        set_track_locked(&mut engine, id, &receipt.track_id, true).expect("lock");
        let document = load_animation_document(&engine);
        assert!(!document.sequence.tracks[0].enabled);
        assert!(document
            .locked_tracks
            .contains(&document.sequence.tracks[0].id));
        assert_eq!(document.sequence.markers[0].id.as_str(), marker);
        assert_eq!(document.sequence.events[0].id.as_str(), event);
        assert!(matches!(
            update_keys(
                &mut engine,
                id,
                &receipt.track_id,
                &[AnimationKeyUpdate {
                    key_id: receipt.key_id,
                    tick: Tick(1),
                    value: None,
                    in_tangent: None,
                    out_tangent: None,
                }],
            ),
            Err(AnimationIntentError::LockedTrack)
        ));
        delete_marker(&mut engine, id, &marker).expect("delete marker");
        delete_event(&mut engine, id, &event).expect("delete event");
        let document = load_animation_document(&engine);
        assert!(document.sequence.markers.is_empty());
        assert!(document.sequence.events.is_empty());
        assert!(engine.undo(), "undo event deletion");
        assert_eq!(load_animation_document(&engine).sequence.events.len(), 1);
    }

    #[test]
    fn same_tick_rekey_only_reuses_and_tombstones_the_current_peers_keys() {
        let mut base = Engine::new(FlecsWorld::new(), 70);
        let id = target(&mut base);
        let snapshot = base.export_updates();
        let mut local = Engine::new(FlecsWorld::new(), 71);
        let mut remote = Engine::new(FlecsWorld::new(), 72);
        local.merge(&snapshot).expect("fork local");
        remote.merge(&snapshot).expect("fork remote");

        let local_receipt = key_property(
            &mut local,
            id,
            "Transform",
            "x",
            Tick(100),
            Interpolation::Linear,
        )
        .expect("local key");
        remote
            .commit(
                "remote pose",
                vec![Op::SetField {
                    entity: id,
                    component: "Transform".into(),
                    field: "x".into(),
                    value: FieldValue::Number(8.0),
                }],
            )
            .expect("remote pose");
        let remote_receipt = key_property(
            &mut remote,
            id,
            "Transform",
            "x",
            Tick(100),
            Interpolation::Linear,
        )
        .expect("remote key");
        local
            .merge(&remote.export_updates())
            .expect("merge remote key");

        local
            .commit(
                "local pose",
                vec![Op::SetField {
                    entity: id,
                    component: "Transform".into(),
                    field: "x".into(),
                    value: FieldValue::Number(12.0),
                }],
            )
            .expect("local pose");
        let rekeyed = key_property(
            &mut local,
            id,
            "Transform",
            "x",
            Tick(100),
            Interpolation::Linear,
        )
        .expect("re-key locally");

        assert_eq!(rekeyed.key_id, local_receipt.key_id);
        assert_eq!(rekeyed.replaced_keys, 1);
        let track = &load_animation_document(&local).sequence.tracks[0];
        assert_eq!(track.keyframes.len(), 2, "remote choice must survive");
        assert_eq!(
            track
                .keyframes
                .iter()
                .find(|key| key.id.as_str() == remote_receipt.key_id)
                .expect("remote key retained")
                .value,
            AnimValue::Number(8.0)
        );
        assert_eq!(
            animation_fields(&local, id).get(&key_semantic_field(
                &local_receipt.track_id,
                &remote_receipt.key_id,
                "active"
            )),
            Some(&FieldValue::Bool(true)),
            "local re-key must not tombstone a peer's element"
        );
    }

    #[test]
    fn multi_key_delete_validates_every_target_and_undoes_as_one_commit() {
        let mut engine = Engine::new(FlecsWorld::new(), 73);
        let first_entity = target(&mut engine);
        let second_entity = target(&mut engine);
        let first = key_property(
            &mut engine,
            first_entity,
            "Transform",
            "x",
            Tick(10),
            Interpolation::Linear,
        )
        .expect("first key");
        let second = key_property(
            &mut engine,
            second_entity,
            "Transform",
            "x",
            Tick(20),
            Interpolation::Linear,
        )
        .expect("second key");
        let deletes = [
            AnimationKeyDelete {
                entity: first_entity,
                track_id: first.track_id.clone(),
                key_id: first.key_id.clone(),
            },
            AnimationKeyDelete {
                entity: second_entity,
                track_id: second.track_id.clone(),
                key_id: second.key_id.clone(),
            },
        ];

        set_track_locked(&mut engine, second_entity, &second.track_id, true).expect("lock second");
        let before_rejected_edit = authored_animation_revision(&engine);
        assert_eq!(
            delete_keys(&mut engine, &deletes),
            Err(AnimationIntentError::LockedTrack)
        );
        assert_eq!(
            authored_animation_revision(&engine),
            before_rejected_edit,
            "validation failure must not partially tombstone earlier keys"
        );

        set_track_locked(&mut engine, second_entity, &second.track_id, false)
            .expect("unlock second");
        assert_eq!(delete_keys(&mut engine, &deletes), Ok(2));
        assert!(load_animation_document(&engine).sequence.tracks.is_empty());
        assert!(engine.undo(), "one undo restores the atomic batch");
        let restored = load_animation_document(&engine);
        assert_eq!(restored.sequence.tracks.len(), 2);
        assert_eq!(
            restored
                .sequence
                .tracks
                .iter()
                .map(|track| track.keyframes.len())
                .sum::<usize>(),
            2
        );
    }

    #[test]
    fn authored_revision_covers_mute_lock_and_key_tombstones() {
        let mut engine = Engine::new(FlecsWorld::new(), 74);
        let id = target(&mut engine);
        let empty = authored_animation_revision(&engine);
        let key = key_property(
            &mut engine,
            id,
            "Transform",
            "x",
            Tick(10),
            Interpolation::Linear,
        )
        .expect("key");
        let authored = authored_animation_revision(&engine);
        assert_ne!(empty, authored);

        set_track_enabled(&mut engine, id, &key.track_id, false).expect("mute");
        let muted = authored_animation_revision(&engine);
        assert_ne!(authored, muted, "disabled metadata participates");
        set_track_enabled(&mut engine, id, &key.track_id, true).expect("unmute");
        assert_eq!(authored_animation_revision(&engine), authored);

        set_track_locked(&mut engine, id, &key.track_id, true).expect("lock");
        let locked = authored_animation_revision(&engine);
        assert_ne!(authored, locked, "editor lock metadata participates");
        set_track_locked(&mut engine, id, &key.track_id, false).expect("unlock");
        assert_eq!(authored_animation_revision(&engine), authored);

        delete_key(&mut engine, id, &key.track_id, &key.key_id).expect("delete");
        let tombstoned = authored_animation_revision(&engine);
        assert_ne!(authored, tombstoned, "inactive key tombstones participate");
        assert!(load_animation_document(&engine).sequence.tracks.is_empty());
        assert!(engine.undo(), "restore tombstone");
        assert_eq!(authored_animation_revision(&engine), authored);

        engine
            .commit(
                "legacy joint timeline",
                vec![Op::SetField {
                    entity: id,
                    component: JOINT_TRACK.into(),
                    field: "keys".into(),
                    value: FieldValue::Str("0:0;1:1".into()),
                }],
            )
            .expect("legacy timeline field");
        assert_ne!(
            authored_animation_revision(&engine),
            authored,
            "legacy JointTrack authoring participates until migration"
        );
    }
}
