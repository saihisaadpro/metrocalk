use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{ClipId, SequenceId, Severity, Tick, TimeBase, ValueKind};

pub const ANIMATION_MANIFEST_VERSION: u32 = 1;
/// Version of the project-repository record that wraps an immutable animation manifest.
///
/// This is intentionally separate from [`ANIMATION_MANIFEST_VERSION`]. The manifest remains the
/// portable, playback-facing v1 payload (including its existing bincode layout), while repositories can
/// evolve logical names, revisions and import bookkeeping without changing authored animation bytes.
pub const ANIMATION_ASSET_RECORD_VERSION: u32 = 1;

macro_rules! manifest_id {
    ($name:ident) => {
        #[derive(
            Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

manifest_id!(AnimationAssetId);
manifest_id!(AnimationFacetId);
manifest_id!(AnimationRevisionId);
manifest_id!(AnimationDependencyId);
// Logical ID of any engine asset referenced by animation (rig, mesh, audio, script, UI document, ...).
manifest_id!(ReferencedAssetId);

/// Original source identity, separate from processed content identity so re-import can explain change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceIdentity {
    pub uri: String,
    pub source_hash: String,
    pub importer: String,
    pub importer_version: String,
    #[serde(default)]
    pub modified_epoch_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentIdentity {
    pub content_hash: String,
    pub canonical_hash: String,
    pub byte_size: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationCapability {
    TransformTracks,
    PropertyTracks,
    SkeletalPose,
    MorphTargets,
    RootMotion,
    Events,
    Contacts,
    CubicInterpolation,
    ReversePlayback,
    StringProperties,
    PhysicsSynchronization,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipSummary {
    pub clip_id: ClipId,
    pub sequence_id: SequenceId,
    pub name: String,
    pub duration: Tick,
    pub time_base: TimeBase,
    pub track_count: u32,
    pub event_count: u32,
    pub marker_count: u32,
    #[serde(default)]
    pub animated_value_kinds: BTreeSet<ValueKind>,
}

/// Canonical rig identity. Names and parents support transparent compatibility diagnostics instead of a
/// single opaque yes/no hash failure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkeletonSignature {
    pub signature_hash: String,
    pub rest_pose_hash: String,
    pub joint_names: Vec<String>,
    pub parent_indices: Vec<Option<u32>>,
    #[serde(default)]
    pub humanoid_profile: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkeletonMatchPolicy {
    #[default]
    Exact,
    NamedSubset,
    HumanoidRetargetable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkeletonCompatibility {
    pub policy: SkeletonMatchPolicy,
    pub required_signature_hash: String,
    #[serde(default)]
    pub required_joints: BTreeSet<String>,
    #[serde(default)]
    pub required_humanoid_profile: Option<String>,
    #[serde(default)]
    pub retarget_profile: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RootMotionMetadata {
    pub present: bool,
    #[serde(default)]
    pub source_joint: Option<String>,
    pub translation_axes: [bool; 3],
    pub extracts_yaw: bool,
    pub average_speed_metres_per_second: f64,
    pub in_place_preview_supported: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactMetadata {
    pub name: String,
    pub joint: String,
    pub start: Tick,
    pub end: Tick,
    pub semantic: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMetadata {
    pub names: BTreeSet<String>,
    pub payload_kinds: BTreeSet<ValueKind>,
    pub interval_crossing_safe: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MorphMetadata {
    pub target_count: u32,
    pub target_names: Vec<String>,
    pub minimum_weight: f64,
    pub maximum_weight: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionMetadata {
    pub namespace: String,
    pub version: u32,
    pub required: bool,
    pub payload_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompressionMetadata {
    pub codec: String,
    pub codec_version: String,
    pub source_bytes: u64,
    pub compressed_bytes: u64,
    pub maximum_translation_error_metres: f64,
    pub maximum_rotation_error_degrees: f64,
    pub maximum_scale_error: f64,
    pub measured_sample_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReimportMetadata {
    pub strategy: String,
    pub previous_source_hash: Option<String>,
    pub preserves_authored_overrides: bool,
    /// Stable imported channel identity -> current project TrackId string.
    pub channel_bindings: BTreeMap<String, String>,
    pub orphaned_bindings: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeterminismMetadata {
    pub compiler_version: String,
    pub canonicalization_version: u32,
    pub compiled_hash: String,
    pub integer_tick_time: bool,
    pub stable_ordering: bool,
    pub deterministic_cpu_evaluation: bool,
}

/// One animation capability facet of a possibly multi-facet asset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationFacet {
    pub id: AnimationFacetId,
    pub capabilities: BTreeSet<AnimationCapability>,
    pub clips: Vec<ClipSummary>,
    #[serde(default)]
    pub skeleton_signature: Option<SkeletonSignature>,
    #[serde(default)]
    pub skeleton_compatibility: Option<SkeletonCompatibility>,
    pub root_motion: RootMotionMetadata,
    #[serde(default)]
    pub contacts: Vec<ContactMetadata>,
    pub events: EventMetadata,
    #[serde(default)]
    pub morphs: Option<MorphMetadata>,
    #[serde(default)]
    pub extensions: Vec<ExtensionMetadata>,
    #[serde(default)]
    pub compression: Option<CompressionMetadata>,
    pub reimport: ReimportMetadata,
    pub determinism: DeterminismMetadata,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationAssetManifest {
    pub schema_version: u32,
    pub asset_id: AnimationAssetId,
    pub display_name: String,
    pub source: SourceIdentity,
    pub content: ContentIdentity,
    pub facets: Vec<AnimationFacet>,
}

/// One immutable cooked revision behind a stable logical asset identity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationRevisionMetadata {
    #[serde(default)]
    pub id: AnimationRevisionId,
    #[serde(default)]
    pub ordinal: u64,
    #[serde(default)]
    pub parent: Option<AnimationRevisionId>,
    #[serde(default)]
    pub source_hash: String,
    #[serde(default)]
    pub content_hash: String,
    #[serde(default)]
    pub canonical_hash: String,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AnimationDependencyKind {
    #[default]
    Other,
    SourceBlob,
    Mesh,
    Skeleton,
    Material,
    Audio,
    Script,
    UiDocument,
    Extension,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AnimationDependencyRequirement {
    #[default]
    Required,
    Optional,
    EditorOnly,
}

/// A logical asset-to-asset edge. Resolution is deliberately repository-owned rather than persisted here:
/// a checked-in record must not change merely because a collaborator temporarily lacks a dependency.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationAssetDependency {
    pub id: AnimationDependencyId,
    pub asset_id: ReferencedAssetId,
    #[serde(default)]
    pub kind: AnimationDependencyKind,
    #[serde(default)]
    pub requirement: AnimationDependencyRequirement,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub expected_revision: Option<AnimationRevisionId>,
    #[serde(default)]
    pub expected_content_hash: Option<String>,
}

/// Import state for the currently selected cooked revision. A failed re-import may retain a usable previous
/// revision; only `Failed` means there is no revision safe to play.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AnimationImportState {
    #[default]
    Unknown,
    Pending,
    Importing,
    Ready,
    ReadyWithWarnings,
    Stale,
    FailedRetainingPrevious,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationImportStatus {
    #[serde(default)]
    pub state: AnimationImportState,
    #[serde(default)]
    pub attempt: u64,
    #[serde(default)]
    pub last_successful_source_hash: Option<String>,
    #[serde(default)]
    pub pending_source_hash: Option<String>,
    #[serde(default)]
    pub summary: String,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AnimationEditorContext {
    #[default]
    Generic,
    TwoD,
    ThreeD,
    Ui,
    Cad,
}

/// Stable hints used by an asset browser/inspector. They do not participate in playback compilation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationEditorRepresentation {
    #[serde(default)]
    pub primary_context: AnimationEditorContext,
    #[serde(default = "default_editor_contexts")]
    pub supported_contexts: BTreeSet<AnimationEditorContext>,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub preview_clip: Option<ClipId>,
}

impl Default for AnimationEditorRepresentation {
    fn default() -> Self {
        Self {
            primary_context: AnimationEditorContext::Generic,
            supported_contexts: default_editor_contexts(),
            icon: String::new(),
            preview_clip: None,
        }
    }
}

fn default_editor_contexts() -> BTreeSet<AnimationEditorContext> {
    BTreeSet::from([AnimationEditorContext::Generic])
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AnimationSourceLocation {
    #[default]
    Unknown,
    Embedded,
    ProjectRelative,
    ExternalFile,
    Generated,
    Remote,
}

/// Repository-facing source location. `project_relative_path` is the portable rename/move/watch address;
/// `original_uri` remains evidence and must never be used as the logical asset identity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationSourceReference {
    #[serde(default)]
    pub location: AnimationSourceLocation,
    #[serde(default)]
    pub project_relative_path: Option<String>,
    #[serde(default)]
    pub original_uri: Option<String>,
    #[serde(default)]
    pub watch_for_changes: bool,
    #[serde(default)]
    pub last_observed_hash: Option<String>,
}

/// A retained re-import outcome. Channel-local identity is optional so whole-source failures use the same
/// diagnostic shape without inventing a fake binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationReimportDiagnostic {
    pub severity: Severity,
    pub code: String,
    #[serde(default)]
    pub channel_id: Option<String>,
    pub message: String,
    pub remediation: String,
    #[serde(default)]
    pub previous_revision: Option<AnimationRevisionId>,
    #[serde(default)]
    pub candidate_source_hash: Option<String>,
}

/// Project-scoped lifecycle metadata around an immutable [`AnimationAssetManifest`].
///
/// Keeping this as a wrapper is a compatibility guarantee: a v1 bare manifest continues to deserialize
/// byte-for-byte as before. Self-describing repository formats can deserialize a record containing only
/// `manifest`; every added lifecycle field has a safe default and effective identities fall back to the
/// manifest's existing stable content identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationAssetRecord {
    #[serde(default = "animation_asset_record_version")]
    pub record_schema_version: u32,
    #[serde(default)]
    pub logical_id: AnimationAssetId,
    #[serde(default)]
    pub revision: AnimationRevisionMetadata,
    #[serde(default)]
    pub dependencies: Vec<AnimationAssetDependency>,
    #[serde(default)]
    pub import_status: AnimationImportStatus,
    #[serde(default)]
    pub editor: AnimationEditorRepresentation,
    #[serde(default)]
    pub source: AnimationSourceReference,
    #[serde(default)]
    pub reimport_diagnostics: Vec<AnimationReimportDiagnostic>,
    pub manifest: AnimationAssetManifest,
}

const fn animation_asset_record_version() -> u32 {
    ANIMATION_ASSET_RECORD_VERSION
}

impl From<AnimationAssetManifest> for AnimationAssetRecord {
    fn from(manifest: AnimationAssetManifest) -> Self {
        let has_manifest_errors = manifest
            .quality_diagnostics()
            .iter()
            .any(|item| item.severity == Severity::Error);
        let logical_id = manifest.asset_id.clone();
        let revision = AnimationRevisionMetadata {
            id: AnimationRevisionId::new(format!("revision:{}", manifest.content.canonical_hash)),
            ordinal: 1,
            parent: None,
            source_hash: manifest.source.source_hash.clone(),
            content_hash: manifest.content.content_hash.clone(),
            canonical_hash: manifest.content.canonical_hash.clone(),
        };
        let source = source_reference(&manifest.source);
        let import_status = AnimationImportStatus {
            state: if has_manifest_errors {
                AnimationImportState::Failed
            } else {
                AnimationImportState::Ready
            },
            attempt: 1,
            last_successful_source_hash: (!has_manifest_errors)
                .then(|| manifest.source.source_hash.clone()),
            pending_source_hash: has_manifest_errors.then(|| manifest.source.source_hash.clone()),
            summary: if has_manifest_errors {
                "manifest validation failed".into()
            } else {
                "imported and validated".into()
            },
        };
        Self {
            record_schema_version: ANIMATION_ASSET_RECORD_VERSION,
            logical_id,
            revision,
            dependencies: Vec::new(),
            import_status,
            editor: AnimationEditorRepresentation::default(),
            source,
            reimport_diagnostics: Vec::new(),
            manifest,
        }
    }
}

impl AnimationAssetRecord {
    /// Stable repository identity. A minimally-wrapped legacy manifest falls back to its existing asset ID.
    #[must_use]
    pub fn effective_logical_id(&self) -> &AnimationAssetId {
        if self.logical_id.as_str().trim().is_empty() {
            &self.manifest.asset_id
        } else {
            &self.logical_id
        }
    }

    /// Stable cooked-revision identity. Legacy records derive it from the canonical content hash.
    #[must_use]
    pub fn effective_revision_id(&self) -> AnimationRevisionId {
        if self.revision.id.as_str().trim().is_empty() {
            AnimationRevisionId::new(format!("revision:{}", self.manifest.content.canonical_hash))
        } else {
            self.revision.id.clone()
        }
    }

    /// Whether the current record has a usable, compatible revision for this playback target.
    #[must_use]
    pub fn can_play_on(&self, target: &PlaybackTarget) -> bool {
        !self.has_fatal_readiness_error() && self.manifest.can_play_on(target)
    }

    #[must_use]
    pub fn has_fatal_readiness_error(&self) -> bool {
        self.quality_diagnostics()
            .iter()
            .any(|item| item.severity == Severity::Error)
    }

    /// Manifest + lifecycle diagnostics, deterministically ordered for stable UI and source-control output.
    #[must_use]
    pub fn quality_diagnostics(&self) -> Vec<QualityDiagnostic> {
        let mut diagnostics = self.manifest.quality_diagnostics();
        append_identity_diagnostics(self, &mut diagnostics);
        append_dependency_diagnostics(self, &mut diagnostics);
        append_lifecycle_diagnostics(self, &mut diagnostics);
        append_reimport_diagnostics(self, &mut diagnostics);
        diagnostics.sort_by(|left, right| {
            left.location
                .cmp(&right.location)
                .then(left.code.cmp(&right.code))
                .then(left.message.cmp(&right.message))
        });
        diagnostics
    }

    /// Deterministic repository fingerprint. Collection insertion order and diagnostic arrival order do not
    /// affect it; volatile resolution/cache state is intentionally absent from the record model.
    #[must_use]
    pub fn repository_fingerprint(&self) -> String {
        let mut canonical = Vec::new();
        push_u64(&mut canonical, u64::from(self.record_schema_version));
        push_str(&mut canonical, self.effective_logical_id().as_str());
        push_str(&mut canonical, self.effective_revision_id().as_str());
        push_u64(&mut canonical, self.revision.ordinal);
        push_option_id(&mut canonical, self.revision.parent.as_ref());
        push_str(&mut canonical, &self.revision.source_hash);
        push_str(&mut canonical, &self.revision.content_hash);
        push_str(&mut canonical, &self.revision.canonical_hash);
        push_u64(&mut canonical, u64::from(self.manifest.schema_version));
        push_str(&mut canonical, &self.manifest.display_name);
        push_str(&mut canonical, &self.manifest.source.source_hash);
        push_str(&mut canonical, &self.manifest.content.content_hash);
        push_str(&mut canonical, &self.manifest.content.canonical_hash);
        let mut dependencies: Vec<_> = self.dependencies.iter().collect();
        dependencies.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then(left.asset_id.cmp(&right.asset_id))
                .then(left.role.cmp(&right.role))
                .then(left.kind.cmp(&right.kind))
                .then(left.requirement.cmp(&right.requirement))
                .then(left.expected_revision.cmp(&right.expected_revision))
                .then(left.expected_content_hash.cmp(&right.expected_content_hash))
        });
        for item in dependencies {
            push_str(&mut canonical, item.id.as_str());
            push_str(&mut canonical, item.asset_id.as_str());
            push_str(&mut canonical, dependency_kind_name(item.kind));
            push_str(
                &mut canonical,
                dependency_requirement_name(item.requirement),
            );
            push_str(&mut canonical, &item.role);
            push_option_id(&mut canonical, item.expected_revision.as_ref());
            push_option_str(&mut canonical, item.expected_content_hash.as_deref());
        }
        push_str(&mut canonical, import_state_name(self.import_status.state));
        push_u64(&mut canonical, self.import_status.attempt);
        push_option_str(
            &mut canonical,
            self.import_status.last_successful_source_hash.as_deref(),
        );
        push_option_str(
            &mut canonical,
            self.import_status.pending_source_hash.as_deref(),
        );
        push_str(&mut canonical, &self.import_status.summary);
        push_str(
            &mut canonical,
            editor_context_name(self.editor.primary_context),
        );
        for context in &self.editor.supported_contexts {
            push_str(&mut canonical, editor_context_name(*context));
        }
        push_str(&mut canonical, &self.editor.icon);
        push_option_str(
            &mut canonical,
            self.editor.preview_clip.as_ref().map(ClipId::as_str),
        );
        push_str(&mut canonical, source_location_name(self.source.location));
        push_option_str(&mut canonical, self.source.project_relative_path.as_deref());
        push_option_str(&mut canonical, self.source.original_uri.as_deref());
        canonical.push(u8::from(self.source.watch_for_changes));
        push_option_str(&mut canonical, self.source.last_observed_hash.as_deref());
        let mut reimport: Vec<_> = self.reimport_diagnostics.iter().collect();
        reimport.sort_by(|left, right| {
            left.code
                .cmp(&right.code)
                .then(left.channel_id.cmp(&right.channel_id))
                .then(left.message.cmp(&right.message))
                .then(left.severity.cmp(&right.severity))
                .then(left.remediation.cmp(&right.remediation))
                .then(left.previous_revision.cmp(&right.previous_revision))
                .then(left.candidate_source_hash.cmp(&right.candidate_source_hash))
        });
        for item in reimport {
            push_str(&mut canonical, severity_name(item.severity));
            push_str(&mut canonical, &item.code);
            push_option_str(&mut canonical, item.channel_id.as_deref());
            push_str(&mut canonical, &item.message);
            push_str(&mut canonical, &item.remediation);
            push_option_id(&mut canonical, item.previous_revision.as_ref());
            push_option_str(&mut canonical, item.candidate_source_hash.as_deref());
        }
        format!("mtkanim-record:{:032x}", fnv1a_128(&canonical))
    }
}

fn append_identity_diagnostics(
    record: &AnimationAssetRecord,
    diagnostics: &mut Vec<QualityDiagnostic>,
) {
    if record.record_schema_version != ANIMATION_ASSET_RECORD_VERSION {
        diagnostics.push(diagnostic(
            Severity::Error,
            QualityCode::UnsupportedAssetRecordVersion,
            "record.record_schema_version",
            format!(
                "asset record version {} is unsupported",
                record.record_schema_version
            ),
            "Migrate the repository record before editing or playback.",
        ));
    }
    if record.effective_logical_id().as_str().trim().is_empty() {
        diagnostics.push(diagnostic(
            Severity::Error,
            QualityCode::MissingLogicalAssetIdentity,
            "record.logical_id",
            "logical animation asset identity is missing",
            "Assign a project-stable logical ID independent of content hashes and file paths.",
        ));
    }
    if revision_is_legacy_default(&record.revision) {
        return;
    }
    if record.revision.id.as_str().trim().is_empty() {
        diagnostics.push(diagnostic(
            Severity::Error,
            QualityCode::MissingRevisionIdentity,
            "record.revision.id",
            "an explicit revision has no stable identity",
            "Assign a revision ID before publishing the cooked asset.",
        ));
    }
    check_revision_hash(
        diagnostics,
        "source_hash",
        &record.revision.source_hash,
        &record.manifest.source.source_hash,
    );
    check_revision_hash(
        diagnostics,
        "content_hash",
        &record.revision.content_hash,
        &record.manifest.content.content_hash,
    );
    check_revision_hash(
        diagnostics,
        "canonical_hash",
        &record.revision.canonical_hash,
        &record.manifest.content.canonical_hash,
    );
}

fn append_dependency_diagnostics(
    record: &AnimationAssetRecord,
    diagnostics: &mut Vec<QualityDiagnostic>,
) {
    let mut dependency_ids = BTreeSet::new();
    for (index, dependency) in record.dependencies.iter().enumerate() {
        let location = format!("record.dependencies[{index}]");
        if dependency.id.as_str().trim().is_empty()
            || dependency.asset_id.as_str().trim().is_empty()
            || dependency.role.trim().is_empty()
        {
            diagnostics.push(diagnostic(
                Severity::Error,
                QualityCode::InvalidDependency,
                &location,
                "dependency identity, target, or role is missing",
                "Provide a stable edge ID, logical target asset ID, and semantic role.",
            ));
        }
        if !dependency_ids.insert(dependency.id.as_str()) {
            diagnostics.push(diagnostic(
                Severity::Error,
                QualityCode::DuplicateDependencyIdentity,
                &location,
                format!("dependency ID '{}' is duplicated", dependency.id.as_str()),
                "Give every dependency edge a unique stable ID.",
            ));
        }
        if dependency.asset_id.as_str() == record.effective_logical_id().as_str() {
            diagnostics.push(diagnostic(
                Severity::Error,
                QualityCode::CyclicDependency,
                &location,
                "animation asset directly depends on itself",
                "Remove the self-reference or extract the shared data into another asset.",
            ));
        }
    }
}

fn append_lifecycle_diagnostics(
    record: &AnimationAssetRecord,
    diagnostics: &mut Vec<QualityDiagnostic>,
) {
    match record.import_status.state {
        AnimationImportState::Failed => diagnostics.push(diagnostic(
            Severity::Error,
            QualityCode::ImportStateBlocksPlayback,
            "record.import_status",
            "the import failed and no usable cooked revision is available",
            "Repair the source or restore a known-good revision before playback.",
        )),
        AnimationImportState::FailedRetainingPrevious => diagnostics.push(diagnostic(
            Severity::Warning,
            QualityCode::ImportStateNeedsAttention,
            "record.import_status",
            "the latest import failed; the previous cooked revision remains active",
            "Review the import diagnostics and retry without discarding the last good revision.",
        )),
        AnimationImportState::Stale => diagnostics.push(diagnostic(
            Severity::Warning,
            QualityCode::ImportStateNeedsAttention,
            "record.import_status",
            "the source changed after the active cooked revision was produced",
            "Review and re-import the source, preserving authored overrides.",
        )),
        AnimationImportState::Unknown
        | AnimationImportState::Pending
        | AnimationImportState::Importing
        | AnimationImportState::Ready
        | AnimationImportState::ReadyWithWarnings => {}
    }
    if !record
        .editor
        .supported_contexts
        .contains(&record.editor.primary_context)
    {
        diagnostics.push(diagnostic(
            Severity::Error,
            QualityCode::EditorContextMismatch,
            "record.editor.primary_context",
            "primary editor context is not declared as supported",
            "Add the primary context to supported_contexts or choose a supported primary context.",
        ));
    }
    if record
        .source
        .last_observed_hash
        .as_ref()
        .is_some_and(|hash| hash != &record.manifest.source.source_hash)
    {
        diagnostics.push(diagnostic(
            Severity::Warning,
            QualityCode::SourceIdentityMismatch,
            "record.source.last_observed_hash",
            "the observed source hash differs from the active manifest",
            "Stage a re-import and review its channel diff before changing the active revision.",
        ));
    }
}

fn append_reimport_diagnostics(
    record: &AnimationAssetRecord,
    diagnostics: &mut Vec<QualityDiagnostic>,
) {
    for (index, item) in record.reimport_diagnostics.iter().enumerate() {
        diagnostics.push(diagnostic(
            item.severity,
            QualityCode::ReimportDiagnostic,
            &item.channel_id.as_ref().map_or_else(
                || format!("record.reimport_diagnostics[{index}]"),
                |channel| format!("record.reimport_diagnostics[{index}].{channel}"),
            ),
            format!("{}: {}", item.code, item.message),
            item.remediation.clone(),
        ));
    }
}

fn revision_is_legacy_default(revision: &AnimationRevisionMetadata) -> bool {
    revision == &AnimationRevisionMetadata::default()
}

fn check_revision_hash(
    diagnostics: &mut Vec<QualityDiagnostic>,
    field: &str,
    recorded: &str,
    manifest: &str,
) {
    if !recorded.is_empty() && recorded != manifest {
        diagnostics.push(diagnostic(
            Severity::Error,
            QualityCode::RevisionContentMismatch,
            &format!("record.revision.{field}"),
            format!("revision {field} does not match the immutable manifest"),
            "Rebuild the revision record from the validated manifest; do not rewrite content in place.",
        ));
    }
}

fn source_reference(source: &SourceIdentity) -> AnimationSourceReference {
    let uri = source.uri.as_str();
    let (location, project_relative_path) = if let Some(path) = uri.strip_prefix("project://") {
        (
            AnimationSourceLocation::ProjectRelative,
            Some(path.to_owned()),
        )
    } else if uri.starts_with("generation://") {
        (AnimationSourceLocation::Generated, None)
    } else if uri.starts_with("http://") || uri.starts_with("https://") {
        (AnimationSourceLocation::Remote, None)
    } else if uri.starts_with("memory://") {
        (AnimationSourceLocation::Embedded, None)
    } else if uri.is_empty() {
        (AnimationSourceLocation::Unknown, None)
    } else {
        (AnimationSourceLocation::ExternalFile, None)
    };
    AnimationSourceReference {
        location,
        project_relative_path,
        original_uri: (!uri.is_empty()).then(|| uri.to_owned()),
        watch_for_changes: matches!(
            location,
            AnimationSourceLocation::ProjectRelative | AnimationSourceLocation::ExternalFile
        ),
        last_observed_hash: (!source.source_hash.is_empty()).then(|| source.source_hash.clone()),
    }
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_str(out: &mut Vec<u8>, value: &str) {
    push_u64(
        out,
        u64::try_from(value.len()).expect("asset metadata length fits u64"),
    );
    out.extend_from_slice(value.as_bytes());
}

fn push_option_str(out: &mut Vec<u8>, value: Option<&str>) {
    out.push(u8::from(value.is_some()));
    if let Some(value) = value {
        push_str(out, value);
    }
}

fn push_option_id(out: &mut Vec<u8>, value: Option<&AnimationRevisionId>) {
    push_option_str(out, value.map(AnimationRevisionId::as_str));
}

fn fnv1a_128(bytes: &[u8]) -> u128 {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    bytes.iter().fold(OFFSET, |hash, byte| {
        (hash ^ u128::from(*byte)).wrapping_mul(PRIME)
    })
}

fn dependency_kind_name(value: AnimationDependencyKind) -> &'static str {
    match value {
        AnimationDependencyKind::Other => "other",
        AnimationDependencyKind::SourceBlob => "source_blob",
        AnimationDependencyKind::Mesh => "mesh",
        AnimationDependencyKind::Skeleton => "skeleton",
        AnimationDependencyKind::Material => "material",
        AnimationDependencyKind::Audio => "audio",
        AnimationDependencyKind::Script => "script",
        AnimationDependencyKind::UiDocument => "ui_document",
        AnimationDependencyKind::Extension => "extension",
    }
}

fn dependency_requirement_name(value: AnimationDependencyRequirement) -> &'static str {
    match value {
        AnimationDependencyRequirement::Required => "required",
        AnimationDependencyRequirement::Optional => "optional",
        AnimationDependencyRequirement::EditorOnly => "editor_only",
    }
}

fn import_state_name(value: AnimationImportState) -> &'static str {
    match value {
        AnimationImportState::Unknown => "unknown",
        AnimationImportState::Pending => "pending",
        AnimationImportState::Importing => "importing",
        AnimationImportState::Ready => "ready",
        AnimationImportState::ReadyWithWarnings => "ready_with_warnings",
        AnimationImportState::Stale => "stale",
        AnimationImportState::FailedRetainingPrevious => "failed_retaining_previous",
        AnimationImportState::Failed => "failed",
    }
}

fn editor_context_name(value: AnimationEditorContext) -> &'static str {
    match value {
        AnimationEditorContext::Generic => "generic",
        AnimationEditorContext::TwoD => "two_d",
        AnimationEditorContext::ThreeD => "three_d",
        AnimationEditorContext::Ui => "ui",
        AnimationEditorContext::Cad => "cad",
    }
}

fn source_location_name(value: AnimationSourceLocation) -> &'static str {
    match value {
        AnimationSourceLocation::Unknown => "unknown",
        AnimationSourceLocation::Embedded => "embedded",
        AnimationSourceLocation::ProjectRelative => "project_relative",
        AnimationSourceLocation::ExternalFile => "external_file",
        AnimationSourceLocation::Generated => "generated",
        AnimationSourceLocation::Remote => "remote",
    }
}

fn severity_name(value: Severity) -> &'static str {
    match value {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

impl AnimationAssetManifest {
    #[must_use]
    pub fn can_play_on(&self, target: &PlaybackTarget) -> bool {
        !self.facets.is_empty() && self.facets.iter().all(|facet| facet.can_play_on(target))
    }

    #[must_use]
    pub fn missing_capabilities(&self, target: &PlaybackTarget) -> Vec<AnimationCapability> {
        self.facets
            .iter()
            .flat_map(|facet| facet.missing_capabilities(target))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Asset-intrinsic diagnostics suitable for import reports and inspector badges.
    #[must_use]
    pub fn quality_diagnostics(&self) -> Vec<QualityDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.schema_version != ANIMATION_MANIFEST_VERSION {
            diagnostics.push(diagnostic(
                Severity::Error,
                QualityCode::UnsupportedManifestVersion,
                "manifest.schema_version",
                "manifest version is unsupported",
                "Migrate the manifest before loading the asset.",
            ));
        }
        if self.asset_id.0.trim().is_empty() || self.content.content_hash.trim().is_empty() {
            diagnostics.push(diagnostic(
                Severity::Error,
                QualityCode::MissingContentIdentity,
                "manifest.content",
                "stable asset or content identity is missing",
                "Compute persistent asset and content hashes during import.",
            ));
        }
        if self.source.source_hash.trim().is_empty() {
            diagnostics.push(diagnostic(
                Severity::Warning,
                QualityCode::MissingSourceIdentity,
                "manifest.source.source_hash",
                "source hash is missing, so re-import drift cannot be proven",
                "Hash the original source bytes and retain the importer version.",
            ));
        }
        if self.facets.is_empty() {
            diagnostics.push(diagnostic(
                Severity::Error,
                QualityCode::NoAnimationFacet,
                "manifest.facets",
                "asset has no animation facet",
                "Import or author at least one playable animation facet.",
            ));
        }
        for (index, facet) in self.facets.iter().enumerate() {
            diagnostics.extend(facet.quality_diagnostics().into_iter().map(|mut item| {
                item.location = format!("manifest.facets[{index}].{}", item.location);
                item
            }));
        }
        diagnostics.sort_by(|left, right| {
            left.location
                .cmp(&right.location)
                .then(left.code.cmp(&right.code))
        });
        diagnostics
    }
}

impl AnimationFacet {
    #[must_use]
    pub fn missing_capabilities(&self, target: &PlaybackTarget) -> Vec<AnimationCapability> {
        self.capabilities
            .difference(&target.capabilities)
            .copied()
            .collect()
    }

    #[must_use]
    pub fn can_play_on(&self, target: &PlaybackTarget) -> bool {
        self.missing_capabilities(target).is_empty()
            && self.skeleton_matches(target.skeleton.as_ref())
            && self
                .morphs
                .as_ref()
                .is_none_or(|morphs| morphs.target_count <= target.maximum_morph_targets)
            && self
                .extensions
                .iter()
                .filter(|extension| extension.required)
                .all(|extension| target.extensions.contains(&extension.namespace))
    }

    #[must_use]
    pub fn compatibility_diagnostics(&self, target: &PlaybackTarget) -> Vec<QualityDiagnostic> {
        let mut diagnostics = Vec::new();
        for capability in self.missing_capabilities(target) {
            diagnostics.push(diagnostic(
                Severity::Error,
                QualityCode::MissingPlaybackCapability,
                "capabilities",
                format!("playback target does not support {capability:?}"),
                "Enable the capability or choose a compatible playback target.",
            ));
        }
        if !self.skeleton_matches(target.skeleton.as_ref()) {
            diagnostics.push(diagnostic(
                Severity::Error,
                QualityCode::IncompatibleSkeleton,
                "skeleton_compatibility",
                "target skeleton does not satisfy this facet's compatibility policy",
                "Select a compatible rig or configure an explicit retarget profile.",
            ));
        }
        if let Some(morphs) = &self.morphs {
            if morphs.target_count > target.maximum_morph_targets {
                diagnostics.push(diagnostic(
                    Severity::Error,
                    QualityCode::MorphTargetLimitExceeded,
                    "morphs.target_count",
                    format!(
                        "asset needs {} morph targets but the target supports {}",
                        morphs.target_count, target.maximum_morph_targets
                    ),
                    "Raise the target limit or bake/reduce the morph set.",
                ));
            }
        }
        for extension in self
            .extensions
            .iter()
            .filter(|extension| extension.required)
        {
            if !target.extensions.contains(&extension.namespace) {
                diagnostics.push(diagnostic(
                    Severity::Error,
                    QualityCode::MissingRequiredExtension,
                    "extensions",
                    format!(
                        "required extension '{}' is unavailable",
                        extension.namespace
                    ),
                    "Install the extension or convert the asset to core animation features.",
                ));
            }
        }
        diagnostics
    }

    #[must_use]
    pub fn quality_diagnostics(&self) -> Vec<QualityDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.clips.is_empty() {
            diagnostics.push(diagnostic(
                Severity::Error,
                QualityCode::NoClips,
                "clips",
                "facet contains no clips",
                "Add a clip summary for every playable sequence.",
            ));
        }
        diagnostics.extend(skeleton_quality_diagnostics(self));
        if self.root_motion.present && !self.capabilities.contains(&AnimationCapability::RootMotion)
        {
            diagnostics.push(diagnostic(
                Severity::Warning,
                QualityCode::MetadataCapabilityMismatch,
                "root_motion",
                "root-motion metadata is present but the capability is not declared",
                "Declare RootMotion or mark the metadata as absent.",
            ));
        }
        if !self.root_motion.average_speed_metres_per_second.is_finite()
            || self.root_motion.average_speed_metres_per_second < 0.0
        {
            diagnostics.push(diagnostic(
                Severity::Error,
                QualityCode::InvalidQualityMetric,
                "root_motion.average_speed_metres_per_second",
                "average root speed is negative or non-finite",
                "Recompute the metric from finite root transforms.",
            ));
        }
        if let Some(compression) = &self.compression {
            let metrics = [
                compression.maximum_translation_error_metres,
                compression.maximum_rotation_error_degrees,
                compression.maximum_scale_error,
            ];
            if metrics
                .iter()
                .any(|metric| !metric.is_finite() || *metric < 0.0)
            {
                diagnostics.push(diagnostic(
                    Severity::Error,
                    QualityCode::InvalidQualityMetric,
                    "compression",
                    "compression error metrics are negative or non-finite",
                    "Measure finite non-negative errors against the uncompressed source.",
                ));
            }
            if compression.maximum_translation_error_metres > 0.002
                || compression.maximum_rotation_error_degrees > 0.25
            {
                diagnostics.push(diagnostic(
                    Severity::Warning,
                    QualityCode::CompressionErrorHigh,
                    "compression",
                    "compression error exceeds the high-fidelity default budget",
                    "Increase precision or let the user explicitly approve a larger error budget.",
                ));
            }
        }
        if !self.determinism.integer_tick_time
            || !self.determinism.stable_ordering
            || !self.determinism.deterministic_cpu_evaluation
            || self.determinism.compiled_hash.trim().is_empty()
        {
            diagnostics.push(diagnostic(
                Severity::Error,
                QualityCode::DeterminismGuaranteeMissing,
                "determinism",
                "one or more deterministic playback guarantees are missing",
                "Compile with integer ticks, stable ordering, deterministic CPU evaluation, and a hash.",
            ));
        }
        diagnostics
    }

    fn skeleton_matches(&self, target: Option<&SkeletonSignature>) -> bool {
        let Some(requirement) = &self.skeleton_compatibility else {
            return true;
        };
        let Some(target) = target else {
            return false;
        };
        match requirement.policy {
            SkeletonMatchPolicy::Exact => {
                !requirement.required_signature_hash.is_empty()
                    && requirement.required_signature_hash == target.signature_hash
                    && self
                        .skeleton_signature
                        .as_ref()
                        .is_none_or(|source| source.rest_pose_hash == target.rest_pose_hash)
            }
            SkeletonMatchPolicy::NamedSubset => {
                let names: BTreeSet<_> = target.joint_names.iter().cloned().collect();
                requirement.required_joints.is_subset(&names)
            }
            SkeletonMatchPolicy::HumanoidRetargetable => requirement
                .required_humanoid_profile
                .as_ref()
                .is_some_and(|profile| target.humanoid_profile.as_ref() == Some(profile)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackTarget {
    pub capabilities: BTreeSet<AnimationCapability>,
    #[serde(default)]
    pub skeleton: Option<SkeletonSignature>,
    pub maximum_morph_targets: u32,
    #[serde(default)]
    pub extensions: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityCode {
    UnsupportedManifestVersion,
    MissingContentIdentity,
    MissingSourceIdentity,
    NoAnimationFacet,
    NoClips,
    MissingSkeletonSignature,
    InvalidSkeletonSignature,
    MissingPlaybackCapability,
    IncompatibleSkeleton,
    MorphTargetLimitExceeded,
    MissingRequiredExtension,
    MetadataCapabilityMismatch,
    InvalidQualityMetric,
    CompressionErrorHigh,
    DeterminismGuaranteeMissing,
    UnsupportedAssetRecordVersion,
    MissingLogicalAssetIdentity,
    MissingRevisionIdentity,
    RevisionContentMismatch,
    InvalidDependency,
    DuplicateDependencyIdentity,
    CyclicDependency,
    ImportStateBlocksPlayback,
    ImportStateNeedsAttention,
    EditorContextMismatch,
    SourceIdentityMismatch,
    ReimportDiagnostic,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityDiagnostic {
    pub severity: Severity,
    pub code: QualityCode,
    pub location: String,
    pub message: String,
    pub remediation: String,
}

fn skeleton_quality_diagnostics(facet: &AnimationFacet) -> Vec<QualityDiagnostic> {
    let mut diagnostics = Vec::new();
    if facet
        .capabilities
        .contains(&AnimationCapability::SkeletalPose)
        && facet.skeleton_signature.is_none()
    {
        diagnostics.push(diagnostic(
            Severity::Error,
            QualityCode::MissingSkeletonSignature,
            "skeleton_signature",
            "skeletal animation has no skeleton signature",
            "Generate a joint hierarchy and rest-pose signature during import.",
        ));
    }
    let Some(signature) = &facet.skeleton_signature else {
        return diagnostics;
    };
    if !skeleton_signature_is_structurally_valid(signature) {
        diagnostics.push(diagnostic(
            Severity::Error,
            QualityCode::InvalidSkeletonSignature,
            "skeleton_signature",
            "skeleton signature has missing hashes, misaligned joint metadata, an invalid parent, or a parent cycle",
            "Recompute the signature from a complete finite acyclic hierarchy and bind pose.",
        ));
    }
    if facet
        .skeleton_compatibility
        .as_ref()
        .is_some_and(|compatibility| {
            compatibility.policy == SkeletonMatchPolicy::NamedSubset
                && signature.joint_names.iter().collect::<BTreeSet<_>>().len()
                    != signature.joint_names.len()
        })
    {
        diagnostics.push(diagnostic(
            Severity::Error,
            QualityCode::InvalidSkeletonSignature,
            "skeleton_signature.joint_names",
            "named-subset matching is ambiguous because joint display names are duplicated",
            "Use exact canonical hierarchy matching or provide unique semantic joint names before named-subset retargeting.",
        ));
    }
    diagnostics
}

fn skeleton_signature_is_structurally_valid(signature: &SkeletonSignature) -> bool {
    if signature.signature_hash.trim().is_empty()
        || signature.rest_pose_hash.trim().is_empty()
        || signature.joint_names.is_empty()
        || signature.joint_names.len() != signature.parent_indices.len()
        || signature
            .joint_names
            .iter()
            .any(|name| name.trim().is_empty())
    {
        return false;
    }
    let count = signature.joint_names.len();
    for (index, parent) in signature.parent_indices.iter().enumerate() {
        if parent.is_some_and(|parent| {
            usize::try_from(parent).map_or(true, |parent| parent >= count || parent == index)
        }) {
            return false;
        }
        let mut visited = BTreeSet::new();
        let mut cursor = Some(index);
        while let Some(current) = cursor {
            if !visited.insert(current) {
                return false;
            }
            cursor =
                signature.parent_indices[current].and_then(|parent| usize::try_from(parent).ok());
        }
    }
    true
}

fn diagnostic(
    severity: Severity,
    code: QualityCode,
    location: &str,
    message: impl Into<String>,
    remediation: impl Into<String>,
) -> QualityDiagnostic {
    QualityDiagnostic {
        severity,
        code,
        location: location.to_owned(),
        message: message.into(),
        remediation: remediation.into(),
    }
}
