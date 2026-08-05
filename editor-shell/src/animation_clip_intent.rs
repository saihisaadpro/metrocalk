//! Persistent imported-animation clip instances over the authoritative ECS/Loro op stream.
//!
//! Imported clip paths are provenance, not live scene addresses. This intent stores the user's explicit
//! rebinding decision separately from the immutable imported asset. Metadata and every target/full-path
//! mapping use stable granular fields, so one save remains atomic and undoable without hiding the document
//! in an opaque JSON blob.

use std::collections::{BTreeMap, BTreeSet};

use metrocalk_animation::{Binding, BindingRebind, TargetRebind};
use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;
use serde::{Deserialize, Serialize};

pub const ANIMATION_CLIP_INSTANCE: &str = "AnimationClipInstance";
pub const ANIMATION_CLIP_INSTANCE_SCHEMA_VERSION: u32 = 1;

const INSTANCE_PREFIX: &str = "instance";
const SOURCE_BINDING_PREFIX: &str = "source_binding";
const TARGET_REBIND_PREFIX: &str = "target_rebind";
const BINDING_REBIND_PREFIX: &str = "binding_rebind";
const MAX_ID_BYTES: usize = 128;
const MAX_NAME_BYTES: usize = 512;
const MAX_REBINDS: usize = 2_048;

/// Authored provenance and explicit mapping for one immutable imported clip instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationClipInstanceDocument {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub logical_asset_id: String,
    pub revision_id: String,
    pub content_hash: String,
    pub clip_id: String,
    /// Stable source-local clip correspondence key. Unlike `clip_id`, this does not carry the cooked
    /// content revision and can therefore find the same authored clip after an explicit re-import.
    #[serde(default)]
    pub clip_locator: String,
    pub source_sequence_id: String,
    pub expected_source_binding_hash: String,
    /// Typed source evidence captured at the revision the user reviewed. This survives content
    /// replacement so repair UX can show added, removed, and type-changed channels before resaving.
    #[serde(default)]
    pub source_bindings: Vec<Binding>,
    #[serde(default)]
    pub target_rebinds: Vec<TargetRebind>,
    #[serde(default)]
    pub binding_rebinds: Vec<BindingRebind>,
    #[serde(default = "enabled")]
    pub require_rest_pose_match: bool,
}

const fn enabled() -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationClipInstanceIssueSeverity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationClipInstanceIssue {
    pub severity: AnimationClipInstanceIssueSeverity,
    pub code: String,
    pub message: String,
    pub fix: String,
    pub instance_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationClipInstanceLoad {
    pub documents: Vec<AnimationClipInstanceDocument>,
    pub revision: String,
    pub issues: Vec<AnimationClipInstanceIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnimationClipInstanceIntentError {
    StaleRevision { expected: String, actual: String },
    Admission(Vec<AnimationClipInstanceIssue>),
    InstanceNotFound,
    Commit(String),
}

impl std::fmt::Display for AnimationClipInstanceIntentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleRevision { expected, actual } => write!(
                formatter,
                "animation clip instances changed while this edit was open (expected {expected}, now {actual})"
            ),
            Self::Admission(issues) => {
                let summary = issues
                    .iter()
                    .map(|issue| issue.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                write!(formatter, "animation clip instance storage rejected the draft: {summary}")
            }
            Self::InstanceNotFound => {
                formatter.write_str("the animation clip instance no longer exists")
            }
            Self::Commit(reason) => {
                write!(formatter, "the animation clip instance edit was rejected: {reason}")
            }
        }
    }
}

impl std::error::Error for AnimationClipInstanceIntentError {}

/// Load all active clip-instance intents. Corrupt or incomplete records fail closed and remain visible as
/// storage issues; they never become graph sources.
#[must_use]
pub fn load_animation_clip_instances(engine: &Engine<FlecsWorld>) -> AnimationClipInstanceLoad {
    let mut documents = Vec::new();
    let mut issues = Vec::new();
    let mut active_records: BTreeMap<String, Vec<BTreeMap<String, FieldValue>>> = BTreeMap::new();
    for entity in sorted_entity_ids(engine) {
        let fields = component_fields(engine, entity);
        let instance_ids: BTreeSet<_> = fields
            .keys()
            .filter_map(|field| {
                let parts: Vec<_> = field.split("::").collect();
                (parts.len() == 3 && parts[0] == INSTANCE_PREFIX).then(|| parts[1].to_owned())
            })
            .collect();
        for instance_id in instance_ids {
            if record_fields(&fields, INSTANCE_PREFIX, &instance_id, None).get("active")
                == Some(&FieldValue::Bool(true))
            {
                active_records
                    .entry(instance_id)
                    .or_default()
                    .push(fields.clone());
            }
        }
    }
    for (instance_id, records) in active_records {
        if records.len() > 1 {
            issues.push(error(
                "multiple_active_instances",
                format!(
                    "Clip instance '{instance_id}' has {} active storage owners, so its target mapping is ambiguous.",
                    records.len()
                ),
                "Undo or delete the duplicate instance owners before playback.",
                Some(&instance_id),
            ));
            continue;
        }
        if let Some(fields) = records.first() {
            match reconstruct_document(fields, &instance_id) {
                Ok(Some(document)) => {
                    let validation = validate_animation_clip_instance_document(&document);
                    if validation
                        .iter()
                        .any(|issue| issue.severity == AnimationClipInstanceIssueSeverity::Error)
                    {
                        issues.extend(validation);
                    } else {
                        issues.extend(validation);
                        documents.push(document);
                    }
                }
                Ok(None) => {}
                Err(issue) => issues.push(issue),
            }
        }
    }
    documents.sort_by(|left, right| left.id.cmp(&right.id));
    issues.sort_by(|left, right| {
        (
            left.instance_id.as_deref(),
            left.code.as_str(),
            left.message.as_str(),
        )
            .cmp(&(
                right.instance_id.as_deref(),
                right.code.as_str(),
                right.message.as_str(),
            ))
    });
    issues.dedup();
    AnimationClipInstanceLoad {
        documents,
        revision: authored_animation_clip_instance_revision(engine),
        issues,
    }
}

/// Save one complete instance as one atomic transaction. Mapping records are addressed by a stable hash
/// of their source path, so reordering the editor list does not create collaborative churn.
pub fn save_animation_clip_instance(
    engine: &mut Engine<FlecsWorld>,
    expected_revision: &str,
    document: &AnimationClipInstanceDocument,
) -> Result<AnimationClipInstanceLoad, AnimationClipInstanceIntentError> {
    verify_revision(engine, expected_revision)?;
    let active_owners = active_instance_owners(engine, &document.id);
    if active_owners.len() > 1 {
        return Err(AnimationClipInstanceIntentError::Admission(vec![error(
            "multiple_active_instances",
            format!(
                "Clip instance '{}' has {} active storage owners, so saving would be ambiguous.",
                document.id,
                active_owners.len()
            ),
            "Undo or delete the duplicate instance owners before saving.",
            Some(&document.id),
        )]));
    }
    let issues = validate_animation_clip_instance_document(document);
    if issues
        .iter()
        .any(|issue| issue.severity == AnimationClipInstanceIssueSeverity::Error)
    {
        return Err(AnimationClipInstanceIntentError::Admission(issues));
    }
    let existing_owner = instance_owner(engine, &document.id);
    let owner = existing_owner.unwrap_or_else(|| engine.alloc_entity_id());
    let existing = component_fields(engine, owner);
    let desired = desired_fields(document).map_err(AnimationClipInstanceIntentError::Admission)?;
    let mut ops = Vec::new();
    if existing_owner.is_none() {
        ops.extend([
            Op::CreateEntity {
                id: owner,
                parent: None,
            },
            Op::SetField {
                entity: owner,
                component: "__meta__".into(),
                field: "name".into(),
                value: FieldValue::Str(format!("{} (clip instance)", document.name)),
            },
            Op::SetField {
                entity: owner,
                component: "__meta__".into(),
                field: "kind".into(),
                value: FieldValue::Str("animation_clip_instance".into()),
            },
        ]);
    }
    for (field, value) in &desired {
        if existing.get(field) != Some(value) {
            ops.push(set_instance_field(owner, field.clone(), value.clone()));
        }
    }
    for field in existing.keys() {
        if belongs_to_instance(field, &document.id)
            && !desired.contains_key(field)
            && field.ends_with("::active")
            && existing.get(field) != Some(&FieldValue::Bool(false))
        {
            ops.push(set_instance_field(
                owner,
                field.clone(),
                FieldValue::Bool(false),
            ));
        }
    }
    if !ops.is_empty() {
        engine
            .commit("animation-clip-instance-save", ops)
            .map_err(|error| AnimationClipInstanceIntentError::Commit(error.to_string()))?;
    }
    Ok(load_animation_clip_instances(engine))
}

/// Tombstone an instance and every mapping in one undoable transaction.
pub fn delete_animation_clip_instance(
    engine: &mut Engine<FlecsWorld>,
    instance_id: &str,
    expected_revision: &str,
) -> Result<AnimationClipInstanceLoad, AnimationClipInstanceIntentError> {
    verify_revision(engine, expected_revision)?;
    let owner = instance_owner(engine, instance_id)
        .ok_or(AnimationClipInstanceIntentError::InstanceNotFound)?;
    let fields = component_fields(engine, owner);
    let mut ops = Vec::new();
    for (field, value) in fields {
        if belongs_to_instance(&field, instance_id)
            && field.ends_with("::active")
            && value != FieldValue::Bool(false)
        {
            ops.push(set_instance_field(owner, field, FieldValue::Bool(false)));
        }
    }
    if ops.is_empty() {
        return Err(AnimationClipInstanceIntentError::InstanceNotFound);
    }
    engine
        .commit("animation-clip-instance-delete", ops)
        .map_err(|error| AnimationClipInstanceIntentError::Commit(error.to_string()))?;
    Ok(load_animation_clip_instances(engine))
}

/// Canonical authored revision over all granular clip-instance fields.
#[must_use]
pub fn authored_animation_clip_instance_revision(engine: &Engine<FlecsWorld>) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    hash_frame(&mut hash, b"metrocalk-authored-animation-clip-instance-v1");
    for entity in sorted_entity_ids(engine) {
        let fields = component_fields(engine, entity);
        if fields.is_empty() {
            continue;
        }
        hash_frame(&mut hash, entity.to_loro_key().as_bytes());
        for (field, value) in fields {
            hash_frame(&mut hash, field.as_bytes());
            hash_field_value(&mut hash, &value);
        }
    }
    format!("animation-clip-instances:{hash:016x}")
}

#[must_use]
pub fn validate_animation_clip_instance_document(
    document: &AnimationClipInstanceDocument,
) -> Vec<AnimationClipInstanceIssue> {
    let mut issues = Vec::new();
    if document.schema_version != ANIMATION_CLIP_INSTANCE_SCHEMA_VERSION {
        issues.push(error(
            "unsupported_schema",
            format!(
                "Clip instance schema {} is unsupported; expected {}.",
                document.schema_version, ANIMATION_CLIP_INSTANCE_SCHEMA_VERSION
            ),
            "Reopen the setup flow and save the instance in the current schema.",
            Some(&document.id),
        ));
    }
    validate_id(&document.id, "clip instance", &document.id, &mut issues);
    if document.name.trim().is_empty() || document.name.len() > MAX_NAME_BYTES {
        issues.push(error(
            "invalid_name",
            format!("Clip instance name must be non-empty and at most {MAX_NAME_BYTES} bytes."),
            "Choose a short descriptive instance name.",
            Some(&document.id),
        ));
    }
    for (label, value) in [
        ("logical asset", &document.logical_asset_id),
        ("asset revision", &document.revision_id),
        ("content hash", &document.content_hash),
        ("clip", &document.clip_id),
        ("clip locator", &document.clip_locator),
        ("source sequence", &document.source_sequence_id),
        (
            "source binding signature",
            &document.expected_source_binding_hash,
        ),
    ] {
        if value.trim().is_empty() || value.contains('\0') || value.len() > 1_024 {
            issues.push(error(
                "invalid_source_identity",
                format!("{label} identity must be non-empty, bounded, and contain no NUL."),
                "Choose the source from the current imported asset instead of editing provenance.",
                Some(&document.id),
            ));
        }
    }
    if document.source_bindings.len()
        + document.target_rebinds.len()
        + document.binding_rebinds.len()
        > MAX_REBINDS
    {
        issues.push(error(
            "rebind_budget_exceeded",
            format!(
                "Clip instance has {} source-evidence and mapping records; the hard limit is {MAX_REBINDS}.",
                document.source_bindings.len()
                    + document.target_rebinds.len()
                    + document.binding_rebinds.len()
            ),
            "Split the animation into smaller explicit instances.",
            Some(&document.id),
        ));
    }
    let mut source_bindings = BTreeSet::new();
    for binding in &document.source_bindings {
        let path = &binding.path;
        let path_is_valid = [&path.target, &path.component, &path.property]
            .into_iter()
            .chain(path.subpath.iter())
            .all(|segment| {
                !segment.trim().is_empty()
                    && !segment.contains(['/', '\0'])
                    && segment.len() <= 1_024
            });
        if !path_is_valid {
            issues.push(error(
                "invalid_source_binding",
                "Source binding evidence contains an invalid or unbounded path segment.",
                "Reopen setup from the current imported clip instead of editing source evidence.",
                Some(&document.id),
            ));
        }
        if !source_bindings.insert(binding) {
            issues.push(error(
                "duplicate_source_binding",
                format!(
                    "Source binding '{}' is captured more than once.",
                    binding.path.display_path()
                ),
                "Keep one typed evidence record per imported source channel.",
                Some(&document.id),
            ));
        }
    }
    let mut target_sources = BTreeSet::new();
    for mapping in &document.target_rebinds {
        if mapping.source_target.trim().is_empty() || mapping.target_target.trim().is_empty() {
            issues.push(error(
                "invalid_target_rebind",
                "Target mappings require both a source target and a live scene target.",
                "Select a scene entity for every source target before saving.",
                Some(&document.id),
            ));
        }
        if !target_sources.insert(mapping.source_target.as_str()) {
            issues.push(error(
                "duplicate_target_rebind",
                format!(
                    "Source target '{}' is mapped more than once.",
                    mapping.source_target
                ),
                "Keep one unambiguous scene target per imported source target.",
                Some(&document.id),
            ));
        }
    }
    let mut binding_sources = BTreeSet::new();
    for mapping in &document.binding_rebinds {
        if !binding_sources.insert(&mapping.source) {
            issues.push(error(
                "duplicate_binding_rebind",
                format!(
                    "Source binding '{}' is mapped more than once.",
                    mapping.source.path.display_path()
                ),
                "Keep one unambiguous target per complete typed source binding.",
                Some(&document.id),
            ));
        }
    }
    issues
}

fn desired_fields(
    document: &AnimationClipInstanceDocument,
) -> Result<BTreeMap<String, FieldValue>, Vec<AnimationClipInstanceIssue>> {
    desired_fields_with_record_id(document, stable_record_id)
}

fn desired_fields_with_record_id(
    document: &AnimationClipInstanceDocument,
    record_id_for: impl Fn(&[u8]) -> String,
) -> Result<BTreeMap<String, FieldValue>, Vec<AnimationClipInstanceIssue>> {
    let mut fields = BTreeMap::new();
    for (name, value) in [
        ("id", FieldValue::Str(document.id.clone())),
        ("active", FieldValue::Bool(true)),
        (
            "schema_version",
            FieldValue::Integer(i64::from(document.schema_version)),
        ),
        ("name", FieldValue::Str(document.name.clone())),
        (
            "logical_asset_id",
            FieldValue::Str(document.logical_asset_id.clone()),
        ),
        ("revision_id", FieldValue::Str(document.revision_id.clone())),
        (
            "content_hash",
            FieldValue::Str(document.content_hash.clone()),
        ),
        ("clip_id", FieldValue::Str(document.clip_id.clone())),
        (
            "clip_locator",
            FieldValue::Str(document.clip_locator.clone()),
        ),
        (
            "source_sequence_id",
            FieldValue::Str(document.source_sequence_id.clone()),
        ),
        (
            "expected_source_binding_hash",
            FieldValue::Str(document.expected_source_binding_hash.clone()),
        ),
        (
            "require_rest_pose_match",
            FieldValue::Bool(document.require_rest_pose_match),
        ),
    ] {
        fields.insert(instance_field(&document.id, name), value);
    }
    let mut source_binding_records = BTreeMap::new();
    for binding in &document.source_bindings {
        let source = serde_json::to_string(binding).map_err(|error| {
            vec![self_contained_error(
                "binding_serialization_failed",
                format!("Could not serialize source binding evidence: {error}"),
                "Reopen the setup flow and rebuild the mapping.",
                Some(document.id.clone()),
            )]
        })?;
        let record_id = record_id_for(source.as_bytes());
        if source_binding_records
            .insert(record_id.clone(), source.clone())
            .is_some_and(|stored| stored != source)
        {
            return Err(vec![error(
                "mapping_record_id_collision",
                "Two different source binding records produced the same stable storage identity.",
                "Generate a new instance identity and retry; no source evidence was committed.",
                Some(&document.id),
            )]);
        }
        for (name, value) in [
            ("active", FieldValue::Bool(true)),
            ("source", FieldValue::Str(source.clone())),
        ] {
            fields.insert(
                mapping_field(SOURCE_BINDING_PREFIX, &document.id, &record_id, name),
                value,
            );
        }
    }
    let mut target_record_sources = BTreeMap::new();
    for mapping in &document.target_rebinds {
        let record_id = record_id_for(mapping.source_target.as_bytes());
        if target_record_sources
            .insert(record_id.clone(), mapping.source_target.clone())
            .is_some_and(|source| source != mapping.source_target)
        {
            return Err(vec![error(
                "mapping_record_id_collision",
                "Two different target mappings produced the same stable storage identity.",
                "Generate a new instance identity and retry; no mapping was committed.",
                Some(&document.id),
            )]);
        }
        for (name, value) in [
            ("active", FieldValue::Bool(true)),
            (
                "source_target",
                FieldValue::Str(mapping.source_target.clone()),
            ),
            (
                "target_target",
                FieldValue::Str(mapping.target_target.clone()),
            ),
        ] {
            fields.insert(
                mapping_field(TARGET_REBIND_PREFIX, &document.id, &record_id, name),
                value,
            );
        }
    }
    let mut binding_record_sources = BTreeMap::new();
    for mapping in &document.binding_rebinds {
        let source = serde_json::to_string(&mapping.source).map_err(|error| {
            vec![self_contained_error(
                "binding_serialization_failed",
                format!("Could not serialize a source binding: {error}"),
                "Reopen the setup flow and rebuild the mapping.",
                Some(document.id.clone()),
            )]
        })?;
        let target = serde_json::to_string(&mapping.target).map_err(|error| {
            vec![self_contained_error(
                "binding_serialization_failed",
                format!("Could not serialize a target binding: {error}"),
                "Reopen the setup flow and rebuild the mapping.",
                Some(document.id.clone()),
            )]
        })?;
        let record_id = record_id_for(source.as_bytes());
        if binding_record_sources
            .insert(record_id.clone(), source.clone())
            .is_some_and(|stored| stored != source)
        {
            return Err(vec![error(
                "mapping_record_id_collision",
                "Two different full-path mappings produced the same stable storage identity.",
                "Generate a new instance identity and retry; no mapping was committed.",
                Some(&document.id),
            )]);
        }
        for (name, value) in [
            ("active", FieldValue::Bool(true)),
            ("source", FieldValue::Str(source.clone())),
            ("target", FieldValue::Str(target.clone())),
        ] {
            fields.insert(
                mapping_field(BINDING_REBIND_PREFIX, &document.id, &record_id, name),
                value,
            );
        }
    }
    Ok(fields)
}

fn reconstruct_document(
    fields: &BTreeMap<String, FieldValue>,
    instance_id: &str,
) -> Result<Option<AnimationClipInstanceDocument>, AnimationClipInstanceIssue> {
    let metadata = record_fields(fields, INSTANCE_PREFIX, instance_id, None);
    if metadata.get("active") != Some(&FieldValue::Bool(true)) {
        return Ok(None);
    }
    let field = |name: &str| {
        metadata
            .get(name)
            .ok_or_else(|| storage_issue(instance_id, name))
    };
    let string = |name: &str| match field(name)? {
        FieldValue::Str(value) => Ok(value.clone()),
        _ => Err(storage_issue(instance_id, name)),
    };
    let schema_version = match field("schema_version")? {
        FieldValue::Integer(value) => {
            u32::try_from(*value).map_err(|_| storage_issue(instance_id, "schema_version"))?
        }
        _ => return Err(storage_issue(instance_id, "schema_version")),
    };
    let require_rest_pose_match = match field("require_rest_pose_match")? {
        FieldValue::Bool(value) => *value,
        _ => return Err(storage_issue(instance_id, "require_rest_pose_match")),
    };
    let clip_id = string("clip_id")?;
    let clip_locator = match metadata.get("clip_locator") {
        Some(FieldValue::Str(value)) => value.clone(),
        None => clip_id.clone(),
        Some(_) => return Err(storage_issue(instance_id, "clip_locator")),
    };
    let mut source_bindings = Vec::new();
    let mut target_rebinds = Vec::new();
    let mut binding_rebinds = Vec::new();
    let mut records: BTreeSet<(String, String)> = BTreeSet::new();
    for name in fields.keys() {
        let parts: Vec<_> = name.split("::").collect();
        if parts.len() == 4
            && parts[1] == instance_id
            && matches!(
                parts[0],
                SOURCE_BINDING_PREFIX | TARGET_REBIND_PREFIX | BINDING_REBIND_PREFIX
            )
        {
            records.insert((parts[0].to_owned(), parts[2].to_owned()));
        }
    }
    for (prefix, record_id) in records {
        let values = record_fields(fields, &prefix, instance_id, Some(&record_id));
        if values.get("active") != Some(&FieldValue::Bool(true)) {
            continue;
        }
        if prefix == SOURCE_BINDING_PREFIX {
            let source_json = stored_string(&values, "source", instance_id)?;
            let source: Binding = serde_json::from_str(&source_json)
                .map_err(|_| storage_issue(instance_id, "source_binding.source"))?;
            source_bindings.push(source);
        } else if prefix == TARGET_REBIND_PREFIX {
            let source_target = stored_string(&values, "source_target", instance_id)?;
            let target_target = stored_string(&values, "target_target", instance_id)?;
            target_rebinds.push(TargetRebind::new(source_target, target_target));
        } else {
            let source_json = stored_string(&values, "source", instance_id)?;
            let target_json = stored_string(&values, "target", instance_id)?;
            let source: Binding = serde_json::from_str(&source_json)
                .map_err(|_| storage_issue(instance_id, "binding_rebind.source"))?;
            let target: Binding = serde_json::from_str(&target_json)
                .map_err(|_| storage_issue(instance_id, "binding_rebind.target"))?;
            binding_rebinds.push(BindingRebind::new(source, target));
        }
    }
    source_bindings.sort();
    target_rebinds.sort();
    binding_rebinds.sort();
    Ok(Some(AnimationClipInstanceDocument {
        schema_version,
        id: string("id")?,
        name: string("name")?,
        logical_asset_id: string("logical_asset_id")?,
        revision_id: string("revision_id")?,
        content_hash: string("content_hash")?,
        clip_id,
        clip_locator,
        source_sequence_id: string("source_sequence_id")?,
        expected_source_binding_hash: string("expected_source_binding_hash")?,
        source_bindings,
        target_rebinds,
        binding_rebinds,
        require_rest_pose_match,
    }))
}

fn record_fields(
    fields: &BTreeMap<String, FieldValue>,
    prefix: &str,
    instance_id: &str,
    record_id: Option<&str>,
) -> BTreeMap<String, FieldValue> {
    fields
        .iter()
        .filter_map(|(field, value)| {
            let parts: Vec<_> = field.split("::").collect();
            match (record_id, parts.as_slice()) {
                (None, [stored_prefix, stored_instance, name])
                    if *stored_prefix == prefix && *stored_instance == instance_id =>
                {
                    Some(((*name).to_owned(), value.clone()))
                }
                (Some(record), [stored_prefix, stored_instance, stored_record, name])
                    if *stored_prefix == prefix
                        && *stored_instance == instance_id
                        && *stored_record == record =>
                {
                    Some(((*name).to_owned(), value.clone()))
                }
                _ => None,
            }
        })
        .collect()
}

fn stored_string(
    fields: &BTreeMap<String, FieldValue>,
    name: &str,
    instance_id: &str,
) -> Result<String, AnimationClipInstanceIssue> {
    match fields.get(name) {
        Some(FieldValue::Str(value)) => Ok(value.clone()),
        _ => Err(storage_issue(instance_id, name)),
    }
}

fn verify_revision(
    engine: &Engine<FlecsWorld>,
    expected_revision: &str,
) -> Result<(), AnimationClipInstanceIntentError> {
    let actual = authored_animation_clip_instance_revision(engine);
    if expected_revision == actual {
        Ok(())
    } else {
        Err(AnimationClipInstanceIntentError::StaleRevision {
            expected: expected_revision.to_owned(),
            actual,
        })
    }
}

fn instance_owner(engine: &Engine<FlecsWorld>, instance_id: &str) -> Option<EntityId> {
    let active_field = instance_field(instance_id, "active");
    let mut owners: Vec<_> = sorted_entity_ids(engine)
        .into_iter()
        .filter_map(|entity| {
            component_fields(engine, entity)
                .get(&active_field)
                .map(|active| (active != &FieldValue::Bool(true), entity))
        })
        .collect();
    owners.sort();
    owners.into_iter().next().map(|(_, entity)| entity)
}

fn active_instance_owners(engine: &Engine<FlecsWorld>, instance_id: &str) -> Vec<EntityId> {
    let active_field = instance_field(instance_id, "active");
    sorted_entity_ids(engine)
        .into_iter()
        .filter(|entity| {
            component_fields(engine, *entity).get(&active_field) == Some(&FieldValue::Bool(true))
        })
        .collect()
}

fn sorted_entity_ids(engine: &Engine<FlecsWorld>) -> Vec<EntityId> {
    let mut entities = engine.entity_ids();
    entities.sort();
    entities
}

fn component_fields(engine: &Engine<FlecsWorld>, entity: EntityId) -> BTreeMap<String, FieldValue> {
    engine
        .components_of(entity)
        .get(ANIMATION_CLIP_INSTANCE)
        .map(|fields| {
            fields
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn instance_field(instance_id: &str, name: &str) -> String {
    format!("{INSTANCE_PREFIX}::{instance_id}::{name}")
}

fn mapping_field(prefix: &str, instance_id: &str, record_id: &str, name: &str) -> String {
    format!("{prefix}::{instance_id}::{record_id}::{name}")
}

fn belongs_to_instance(field: &str, instance_id: &str) -> bool {
    field.split("::").nth(1) == Some(instance_id)
}

fn set_instance_field(entity: EntityId, field: String, value: FieldValue) -> Op {
    Op::SetField {
        entity,
        component: ANIMATION_CLIP_INSTANCE.into(),
        field,
        value,
    }
}

fn validate_id(
    id: &str,
    kind: &str,
    instance_id: &str,
    issues: &mut Vec<AnimationClipInstanceIssue>,
) {
    let valid = !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if !valid {
        issues.push(error(
            "invalid_id",
            format!(
                "{kind} identity {id:?} must be 1-{MAX_ID_BYTES} ASCII letters, numbers, '.', '_' or '-'."
            ),
            "Generate a stable UUID-style identity for this clip instance.",
            Some(instance_id),
        ));
    }
}

fn storage_issue(instance_id: &str, field: &str) -> AnimationClipInstanceIssue {
    error(
        "incomplete_storage_record",
        format!(
            "Clip instance '{instance_id}' has a missing or incorrectly typed '{field}' field."
        ),
        "Delete the damaged instance and create it again from its current imported clip.",
        Some(instance_id),
    )
}

fn error(
    code: impl Into<String>,
    message: impl Into<String>,
    fix: impl Into<String>,
    instance_id: Option<&str>,
) -> AnimationClipInstanceIssue {
    self_contained_error(code, message, fix, instance_id.map(ToOwned::to_owned))
}

fn self_contained_error(
    code: impl Into<String>,
    message: impl Into<String>,
    fix: impl Into<String>,
    instance_id: Option<String>,
) -> AnimationClipInstanceIssue {
    AnimationClipInstanceIssue {
        severity: AnimationClipInstanceIssueSeverity::Error,
        code: code.into(),
        message: message.into(),
        fix: fix.into(),
        instance_id,
    }
}

fn stable_record_id(bytes: &[u8]) -> String {
    // Two independently domain-separated FNV-1a lanes widen persisted mapping identities to 128 bits.
    // `desired_fields_with_record_id` still detects collisions before insertion rather than treating hash
    // width as permission to overwrite silently.
    let mut high = 0xcbf2_9ce4_8422_2325_u64;
    hash_frame(
        &mut high,
        b"metrocalk-animation-clip-instance-record-v2-high",
    );
    hash_frame(&mut high, bytes);
    let mut low = 0x8422_2325_cbf2_9ce4_u64;
    hash_frame(&mut low, b"metrocalk-animation-clip-instance-record-v2-low");
    hash_frame(&mut low, bytes);
    format!("{high:016x}{low:016x}")
}

fn hash_frame(hash: &mut u64, bytes: &[u8]) {
    *hash ^= u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    *hash = hash.wrapping_mul(0x100_0000_01b3);
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100_0000_01b3);
    }
}

fn hash_field_value(hash: &mut u64, value: &FieldValue) {
    match value {
        FieldValue::Integer(value) => {
            hash_frame(hash, b"integer");
            hash_frame(hash, &value.to_le_bytes());
        }
        FieldValue::Number(value) => {
            hash_frame(hash, b"number");
            hash_frame(hash, &value.to_bits().to_le_bytes());
        }
        FieldValue::Bool(value) => {
            hash_frame(hash, b"bool");
            hash_frame(hash, &[u8::from(*value)]);
        }
        FieldValue::Str(value) => {
            hash_frame(hash, b"string");
            hash_frame(hash, value.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use metrocalk_animation::{PropertyPath, ValueKind};

    use super::*;

    fn binding(target: &str, property: &str, kind: ValueKind) -> Binding {
        Binding {
            path: PropertyPath::new(target, "Transform", property),
            value_kind: kind,
        }
    }

    fn document() -> AnimationClipInstanceDocument {
        AnimationClipInstanceDocument {
            schema_version: ANIMATION_CLIP_INSTANCE_SCHEMA_VERSION,
            id: "clip-instance-1".into(),
            name: "Door open".into(),
            logical_asset_id: "door".into(),
            revision_id: "revision-7".into(),
            content_hash: "sha256:fixture".into(),
            clip_id: "open".into(),
            clip_locator: "source-clip:open".into(),
            source_sequence_id: "sequence-open".into(),
            expected_source_binding_hash: "bindings:fixture".into(),
            source_bindings: vec![binding("gltf-node:3", "scale", ValueKind::Vec3)],
            target_rebinds: vec![TargetRebind::new("gltf-node:3", "10:24")],
            binding_rebinds: vec![BindingRebind::new(
                binding("gltf-node:3", "scale", ValueKind::Vec3),
                binding("10:24", "scale3", ValueKind::Vec3),
            )],
            require_rest_pose_match: true,
        }
    }

    #[test]
    fn save_reload_delete_is_granular_atomic_and_revision_guarded() {
        let mut engine = Engine::new(FlecsWorld::new(), 97);
        let empty_revision = authored_animation_clip_instance_revision(&engine);
        let saved = save_animation_clip_instance(&mut engine, &empty_revision, &document())
            .expect("save instance");
        assert_eq!(saved.documents, vec![document()]);
        assert_ne!(saved.revision, empty_revision);

        let owner = instance_owner(&engine, "clip-instance-1").expect("instance owner");
        let fields = component_fields(&engine, owner);
        assert_eq!(
            fields.get("instance::clip-instance-1::active"),
            Some(&FieldValue::Bool(true))
        );
        assert!(fields
            .keys()
            .any(|field| field.starts_with("target_rebind::clip-instance-1::")));
        assert!(fields
            .keys()
            .any(|field| field.starts_with("binding_rebind::clip-instance-1::")));

        assert!(matches!(
            save_animation_clip_instance(&mut engine, &empty_revision, &document()),
            Err(AnimationClipInstanceIntentError::StaleRevision { .. })
        ));

        let deleted =
            delete_animation_clip_instance(&mut engine, "clip-instance-1", &saved.revision)
                .expect("delete instance");
        assert!(deleted.documents.is_empty());
    }

    #[test]
    fn duplicate_source_mapping_is_rejected_before_commit() {
        let mut engine = Engine::new(FlecsWorld::new(), 98);
        let mut duplicate = document();
        duplicate
            .target_rebinds
            .push(TargetRebind::new("gltf-node:3", "10:25"));
        let revision = authored_animation_clip_instance_revision(&engine);
        let error = save_animation_clip_instance(&mut engine, &revision, &duplicate)
            .expect_err("ambiguous source mapping must fail");
        assert!(matches!(
            error,
            AnimationClipInstanceIntentError::Admission(_)
        ));
        assert!(load_animation_clip_instances(&engine).documents.is_empty());
    }

    #[test]
    fn duplicate_active_storage_owners_fail_closed() {
        let mut engine = Engine::new(FlecsWorld::new(), 99);
        let empty_revision = authored_animation_clip_instance_revision(&engine);
        save_animation_clip_instance(&mut engine, &empty_revision, &document())
            .expect("save first owner");
        let duplicate = engine.alloc_entity_id();
        engine
            .commit(
                "corrupt duplicate owner",
                vec![
                    Op::CreateEntity {
                        id: duplicate,
                        parent: None,
                    },
                    set_instance_field(
                        duplicate,
                        "instance::clip-instance-1::active".into(),
                        FieldValue::Bool(true),
                    ),
                    set_instance_field(
                        duplicate,
                        "instance::clip-instance-1::id".into(),
                        FieldValue::Str("clip-instance-1".into()),
                    ),
                ],
            )
            .expect("seed corrupt duplicate");

        let loaded = load_animation_clip_instances(&engine);
        assert!(loaded.documents.is_empty());
        assert!(loaded
            .issues
            .iter()
            .any(|issue| issue.code == "multiple_active_instances"));
    }

    #[test]
    fn mapping_record_hash_collision_rejects_instead_of_overwriting() {
        let mut colliding = document();
        colliding
            .target_rebinds
            .push(TargetRebind::new("gltf-node:4", "10:25"));
        let issues = desired_fields_with_record_id(&colliding, |_| "forced-collision".into())
            .expect_err("forced collision must fail closed");
        assert!(issues
            .iter()
            .any(|issue| issue.code == "mapping_record_id_collision"));
    }
}
