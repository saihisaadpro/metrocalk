//! Immutable clip instancing and explicit binding rebasing.
//!
//! Imported animation keeps source paths such as `gltf-target:<path-hash>/SkeletonJoint/rotation`. Those paths
//! are useful provenance, but they are not live scene addresses. This module is the format-, ECS- and
//! renderer-independent gate that turns an authored source sequence into a validated immutable instance:
//! every source binding is mapped explicitly, checked against a target catalog, and compiled without
//! mutating the imported asset.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    AnimationAssetId, AnimationRevisionId, Binding, ClipId, CompiledSequence, Sequence, SequenceId,
    SkeletonSignature, ValueKind,
};

/// Version of the canonical typed-binding signature.
pub const BINDING_SIGNATURE_VERSION: u32 = 1;

/// A canonical, order-independent description of the properties a clip reads or writes.
///
/// `bindings` is sorted and de-duplicated. The hash frames every path segment and the value kind, so
/// delimiter-like characters cannot create ambiguous signatures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingSignature {
    pub schema_version: u32,
    pub signature_hash: String,
    pub bindings: Vec<Binding>,
    /// Fixed vector width for bindings whose runtime shape is wider than their [`ValueKind`].
    /// Currently this is required for morph-weight vectors and absent for scalar/fixed-width kinds.
    #[serde(default)]
    pub element_counts: BTreeMap<Binding, u32>,
}

impl BindingSignature {
    /// Build a canonical signature. Collection arrival order never affects the result.
    #[must_use]
    pub fn new(bindings: impl IntoIterator<Item = Binding>) -> Self {
        Self::with_element_counts(bindings, BTreeMap::new())
    }

    /// Build a signature with explicit dynamic-vector widths.
    #[must_use]
    pub fn with_element_counts(
        bindings: impl IntoIterator<Item = Binding>,
        element_counts: BTreeMap<Binding, u32>,
    ) -> Self {
        let mut bindings: Vec<_> = bindings.into_iter().collect();
        bindings.sort();
        bindings.dedup();
        let signature_hash = binding_signature_hash(&bindings, &element_counts);
        Self {
            schema_version: BINDING_SIGNATURE_VERSION,
            signature_hash,
            bindings,
            element_counts,
        }
    }

    /// Signature of enabled, non-empty source tracks (the exact tracks sequence compilation retains).
    #[must_use]
    pub fn from_sequence(sequence: &Sequence) -> Self {
        let tracks: Vec<_> = sequence
            .tracks
            .iter()
            .filter(|track| track.enabled && !track.keyframes.is_empty())
            .collect();
        let element_counts = tracks
            .iter()
            .filter_map(|track| {
                track.keyframes.first().and_then(|key| match &key.value {
                    crate::AnimValue::Weights(values) => u32::try_from(values.len())
                        .ok()
                        .map(|count| (track.binding.clone(), count)),
                    _ => None,
                })
            })
            .collect();
        Self::with_element_counts(
            tracks.into_iter().map(|track| track.binding.clone()),
            element_counts,
        )
    }

    #[must_use]
    pub fn contains(&self, binding: &Binding) -> bool {
        self.bindings.binary_search(binding).is_ok()
    }

    /// Detect a stale, hand-edited, or unsupported serialized signature before it becomes runtime state.
    #[must_use]
    pub fn validation_issues(&self, location: &str) -> Vec<ClipInstantiationIssue> {
        let mut issues = Vec::new();
        if self.schema_version != BINDING_SIGNATURE_VERSION {
            issues.push(issue(
                ClipInstantiationCode::UnsupportedBindingSignatureVersion,
                format!("{location}.schema_version"),
                format!(
                    "binding signature schema {} is unsupported",
                    self.schema_version
                ),
                format!("Rebuild the signature with schema {BINDING_SIGNATURE_VERSION}."),
            ));
        }
        if self.bindings.is_empty() {
            issues.push(issue(
                ClipInstantiationCode::EmptyBindingSignature,
                format!("{location}.bindings"),
                "binding signature contains no playable properties",
                "Provide at least one finite, typed target binding.",
            ));
        }
        for (index, binding) in self.bindings.iter().enumerate() {
            if !valid_binding(binding) {
                issues.push(issue(
                    ClipInstantiationCode::InvalidBinding,
                    format!("{location}.bindings[{index}]"),
                    format!(
                        "binding '{}' contains an empty, slash-bearing, or NUL path segment",
                        binding.path.display_path()
                    ),
                    "Use separate non-empty target, component, property and subpath segments.",
                ));
            }
            match (
                binding.value_kind,
                self.element_counts.get(binding).copied(),
            ) {
                (ValueKind::Weights, None | Some(0)) => issues.push(issue(
                    ClipInstantiationCode::BindingShapeMismatch,
                    format!("{location}.element_counts[{}]", binding.path.display_path()),
                    "morph-weight binding is missing a non-zero element count",
                    "Measure and retain the morph target count in the binding signature.",
                )),
                (ValueKind::Weights, Some(_)) | (_, None) => {}
                (_, Some(_)) => issues.push(issue(
                    ClipInstantiationCode::BindingShapeMismatch,
                    format!("{location}.element_counts[{}]", binding.path.display_path()),
                    "fixed-width binding unexpectedly declares a dynamic element count",
                    "Remove the element count from non-weight bindings.",
                )),
            }
            if index > 0 && self.bindings[index - 1] >= *binding {
                issues.push(issue(
                    ClipInstantiationCode::NonCanonicalBindingSignature,
                    format!("{location}.bindings[{index}]"),
                    "binding signature entries are not strictly sorted and unique",
                    "Rebuild the signature through BindingSignature::new.",
                ));
            }
        }
        if self
            .element_counts
            .keys()
            .any(|binding| self.bindings.binary_search(binding).is_err())
        {
            issues.push(issue(
                ClipInstantiationCode::BindingShapeMismatch,
                format!("{location}.element_counts"),
                "element count metadata references a binding outside the signature",
                "Remove stale shape metadata and rebuild the target catalog.",
            ));
        }
        let expected = binding_signature_hash(&self.bindings, &self.element_counts);
        if self.signature_hash != expected {
            issues.push(issue(
                ClipInstantiationCode::BindingSignatureHashMismatch,
                format!("{location}.signature_hash"),
                "binding signature hash does not match its canonical binding entries",
                "Discard the stale signature and recompute it from the authoritative binding catalog.",
            ));
        }
        issues.sort();
        issues
    }
}

/// Rebind every source property beneath one stable target while retaining component/property/subpath.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TargetRebind {
    pub source_target: String,
    pub target_target: String,
}

impl TargetRebind {
    #[must_use]
    pub fn new(source_target: impl Into<String>, target_target: impl Into<String>) -> Self {
        Self {
            source_target: source_target.into(),
            target_target: target_target.into(),
        }
    }
}

/// Rebind one complete typed property path. This is the precise primitive used when component or
/// property names also change (for example an imported joint channel projected into an ECS sink).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BindingRebind {
    pub source: Binding,
    pub target: Binding,
}

impl BindingRebind {
    #[must_use]
    pub const fn new(source: Binding, target: Binding) -> Self {
        Self { source, target }
    }
}

/// What to do with a source binding that has no full-path or target-level mapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnmappedBindingPolicy {
    /// Production-safe default: source-import identities never leak into a live runtime accidentally.
    #[default]
    Reject,
    /// Retain the source path, but still require that exact path in `target_bindings`.
    KeepSource,
}

/// Immutable provenance supplied by the asset layer for one resolved clip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipInstanceSource {
    pub logical_asset_id: AnimationAssetId,
    pub revision_id: AnimationRevisionId,
    pub content_hash: String,
    pub clip_id: ClipId,
    pub sequence_id: SequenceId,
    #[serde(default)]
    pub skeleton_signature: Option<SkeletonSignature>,
}

/// Caller-authored target contract. The target catalog is authoritative: mapping a path that is not in
/// it fails rather than producing a clip that will silently drop writes at the projection boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipInstantiationRequest {
    pub instance_id: String,
    pub clip_id: ClipId,
    #[serde(default)]
    pub expected_source_binding_hash: Option<String>,
    pub target_bindings: BindingSignature,
    #[serde(default)]
    pub target_rebinds: Vec<TargetRebind>,
    #[serde(default)]
    pub binding_rebinds: Vec<BindingRebind>,
    #[serde(default)]
    pub unmapped_policy: UnmappedBindingPolicy,
    #[serde(default)]
    pub target_skeleton: Option<SkeletonSignature>,
    /// Optional deterministic runtime clock. Imported assets retain their high-resolution source clock
    /// for provenance; an editor/game graph can explicitly normalize every admitted source to its one
    /// shared playhead clock before compilation.
    #[serde(default)]
    pub runtime_time_base: Option<crate::TimeBase>,
    /// Exact imported skeletal playback defaults to matching both hierarchy and bind pose. Disabling this
    /// is explicit because applying local animation onto another bind pose is retargeting, not rebinding.
    #[serde(default = "enabled")]
    pub require_rest_pose_match: bool,
}

impl ClipInstantiationRequest {
    #[must_use]
    pub fn new(
        instance_id: impl Into<String>,
        clip_id: ClipId,
        target_bindings: BindingSignature,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            clip_id,
            expected_source_binding_hash: None,
            target_bindings,
            target_rebinds: Vec::new(),
            binding_rebinds: Vec::new(),
            unmapped_policy: UnmappedBindingPolicy::Reject,
            target_skeleton: None,
            runtime_time_base: None,
            require_rest_pose_match: true,
        }
    }
}

const fn enabled() -> bool {
    true
}

/// Fully validated, immutable source instance ready for the sequence or animation-graph runtime.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompiledClipInstance {
    pub instance_id: String,
    pub source: ClipInstanceSource,
    pub source_binding_signature: BindingSignature,
    /// Signature of the mapped outputs used by this instance, not the caller's wider target catalog.
    pub mapped_binding_signature: BindingSignature,
    /// Canonical complete map, including target-level and identity mappings expanded per source binding.
    pub rebindings: Vec<BindingRebind>,
    /// Canonical compile-fence over source revision, signatures, mappings and immutable sequence plan.
    pub stable_hash: String,
    pub plan: CompiledSequence,
}

impl CompiledClipInstance {
    #[must_use]
    pub const fn plan(&self) -> &CompiledSequence {
        &self.plan
    }

    #[must_use]
    pub fn into_plan(self) -> CompiledSequence {
        self.plan
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipInstantiationCode {
    UnsupportedBindingSignatureVersion,
    EmptyBindingSignature,
    InvalidBinding,
    NonCanonicalBindingSignature,
    BindingSignatureHashMismatch,
    EmptyInstanceId,
    ClipIdentityMismatch,
    SequenceIdentityMismatch,
    MissingClip,
    AmbiguousClip,
    MissingSequence,
    AmbiguousSequence,
    AssetNotReady,
    AssetContractMismatch,
    SourceBindingSignatureMismatch,
    DuplicateTargetRebind,
    DuplicateBindingRebind,
    UnknownSourceTarget,
    UnknownSourceBinding,
    BindingTypeMismatch,
    BindingShapeMismatch,
    MissingTargetBinding,
    UnmappedSourceBinding,
    ConflictingTargetBinding,
    MissingSkeletonSignature,
    InvalidSkeletonSignature,
    MissingTargetSkeleton,
    IncompatibleSkeleton,
    RestPoseMismatch,
    TimeBaseConversionFailed,
    SequenceCompilationFailed,
}

/// One deterministic, user-actionable instancing failure.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClipInstantiationIssue {
    pub code: ClipInstantiationCode,
    pub location: String,
    pub message: String,
    pub remediation: String,
}

impl ClipInstantiationIssue {
    #[must_use]
    pub fn new(
        code: ClipInstantiationCode,
        location: impl Into<String>,
        message: impl Into<String>,
        remediation: impl Into<String>,
    ) -> Self {
        Self {
            code,
            location: location.into(),
            message: message.into(),
            remediation: remediation.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipInstantiationError {
    pub issues: Vec<ClipInstantiationIssue>,
}

impl ClipInstantiationError {
    #[must_use]
    pub fn new(mut issues: Vec<ClipInstantiationIssue>) -> Self {
        issues.sort();
        issues.dedup();
        Self { issues }
    }

    #[must_use]
    pub fn single(issue: ClipInstantiationIssue) -> Self {
        Self::new(vec![issue])
    }
}

impl std::fmt::Display for ClipInstantiationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "animation clip instancing failed with {} issue(s)",
            self.issues.len()
        )
    }
}

impl std::error::Error for ClipInstantiationError {}

/// Validate, rebase and compile one immutable clip instance.
// This is intentionally one audit-friendly, atomic admission gate: it accumulates every deterministic
// mapping diagnostic before it creates or compiles any rebound authored data.
#[allow(clippy::too_many_lines)]
pub fn instantiate_clip(
    source: ClipInstanceSource,
    sequence: &Sequence,
    request: &ClipInstantiationRequest,
) -> Result<CompiledClipInstance, ClipInstantiationError> {
    let source_signature = BindingSignature::from_sequence(sequence);
    let mut issues = request
        .target_bindings
        .validation_issues("request.target_bindings");

    if request.instance_id.trim().is_empty() {
        issues.push(issue(
            ClipInstantiationCode::EmptyInstanceId,
            "request.instance_id",
            "clip instance ID is empty",
            "Assign a persistent, non-empty instance ID.",
        ));
    }
    if request.clip_id != source.clip_id {
        issues.push(issue(
            ClipInstantiationCode::ClipIdentityMismatch,
            "request.clip_id",
            format!(
                "request clip '{}' does not match resolved source clip '{}'",
                request.clip_id, source.clip_id
            ),
            "Refresh the source selection and instance the resolved clip identity.",
        ));
    }
    if sequence.id != source.sequence_id {
        issues.push(issue(
            ClipInstantiationCode::SequenceIdentityMismatch,
            "source.sequence_id",
            format!(
                "resolved sequence '{}' does not match source identity '{}'",
                sequence.id, source.sequence_id
            ),
            "Repair the asset clip-to-sequence index before instancing.",
        ));
    }
    if source.content_hash.trim().is_empty()
        || source.logical_asset_id.as_str().trim().is_empty()
        || source.revision_id.as_str().trim().is_empty()
    {
        issues.push(issue(
            ClipInstantiationCode::AssetContractMismatch,
            "source",
            "clip source is missing logical asset, revision, or content identity",
            "Resolve the clip through a validated animation asset record.",
        ));
    }
    if request
        .runtime_time_base
        .is_some_and(|time_base| time_base.ticks_per_second == 0)
    {
        issues.push(issue(
            ClipInstantiationCode::TimeBaseConversionFailed,
            "request.runtime_time_base",
            "runtime time base has zero ticks per second",
            "Choose one non-zero runtime clock shared by every graph source.",
        ));
    }
    if let Some(expected) = &request.expected_source_binding_hash {
        if expected != &source_signature.signature_hash {
            issues.push(issue(
                ClipInstantiationCode::SourceBindingSignatureMismatch,
                "request.expected_source_binding_hash",
                format!(
                    "expected source binding signature '{expected}', but the clip is '{}'",
                    source_signature.signature_hash
                ),
                "Refresh the mapping against this clip revision before playback.",
            ));
        }
    }
    issues.extend(validate_skeleton_contract(
        &source,
        &source_signature,
        request,
    ));

    let (target_map, exact_map, mapping_issues) =
        validated_mapping_tables(&source_signature, request);
    issues.extend(mapping_issues);

    let mut resolved = Vec::with_capacity(source_signature.bindings.len());
    let mut output_sources: BTreeMap<Binding, Vec<Binding>> = BTreeMap::new();
    for source_binding in &source_signature.bindings {
        let mapped = if let Some(target) = exact_map.get(source_binding) {
            Some((*target).clone())
        } else if let Some(target) = target_map.get(source_binding.path.target.as_str()) {
            let mut target_binding = source_binding.clone();
            target_binding.path.target.clone_from(target);
            Some(target_binding)
        } else if request.unmapped_policy == UnmappedBindingPolicy::KeepSource {
            Some(source_binding.clone())
        } else {
            issues.push(issue(
                ClipInstantiationCode::UnmappedSourceBinding,
                format!("source.bindings[{}]", source_binding.path.display_path()),
                format!(
                    "source binding '{}' has no explicit target mapping",
                    source_binding.path.display_path()
                ),
                "Add a full binding rebind or a target-level rebind for this source.",
            ));
            None
        };
        let Some(mapped) = mapped else {
            continue;
        };
        if mapped.value_kind != source_binding.value_kind {
            issues.push(issue(
                ClipInstantiationCode::BindingTypeMismatch,
                format!(
                    "request.binding_rebinds[{}]",
                    source_binding.path.display_path()
                ),
                format!(
                    "source {:?} binding '{}' maps to incompatible {:?} target '{}'",
                    source_binding.value_kind,
                    source_binding.path.display_path(),
                    mapped.value_kind,
                    mapped.path.display_path()
                ),
                "Map only bindings with the same declared value kind.",
            ));
            continue;
        }
        if source_signature.element_counts.get(source_binding)
            != request.target_bindings.element_counts.get(&mapped)
        {
            issues.push(issue(
                ClipInstantiationCode::BindingShapeMismatch,
                format!("target.bindings[{}]", mapped.path.display_path()),
                format!(
                    "source binding '{}' has element count {:?}, but target '{}' has {:?}",
                    source_binding.path.display_path(),
                    source_signature.element_counts.get(source_binding),
                    mapped.path.display_path(),
                    request.target_bindings.element_counts.get(&mapped)
                ),
                "Map dynamic vector properties only when their measured element counts match.",
            ));
            continue;
        }
        if !request.target_bindings.contains(&mapped) {
            issues.push(issue(
                ClipInstantiationCode::MissingTargetBinding,
                format!("target.bindings[{}]", mapped.path.display_path()),
                format!(
                    "mapped target '{}' ({:?}) is not present in the target binding catalog",
                    mapped.path.display_path(),
                    mapped.value_kind
                ),
                "Refresh the target catalog or choose a binding exposed by the playback target.",
            ));
            continue;
        }
        output_sources
            .entry(mapped.clone())
            .or_default()
            .push(source_binding.clone());
        resolved.push(BindingRebind::new(source_binding.clone(), mapped));
    }

    for (target, sources) in output_sources
        .iter()
        .filter(|(_, sources)| sources.len() > 1)
    {
        issues.push(issue(
            ClipInstantiationCode::ConflictingTargetBinding,
            format!("target.bindings[{}]", target.path.display_path()),
            format!(
                "{} source bindings map to the same target '{}'",
                sources.len(),
                target.path.display_path()
            ),
            "Choose distinct target properties; implicit last-wins projection is not allowed.",
        ));
    }

    if !issues.is_empty() {
        return Err(ClipInstantiationError::new(issues));
    }

    resolved.sort();
    let resolved_by_source: BTreeMap<_, _> = resolved
        .iter()
        .map(|entry| (entry.source.clone(), entry.target.clone()))
        .collect();
    let mut rebound = sequence.clone();
    // The immutable source sequence may be instantiated for many targets. Each plan therefore needs the
    // caller's persistent instance identity as its graph/catalog key; provenance retains the source ID.
    rebound.id = SequenceId::new(request.instance_id.clone());
    for track in rebound
        .tracks
        .iter_mut()
        .filter(|track| track.enabled && !track.keyframes.is_empty())
    {
        track.binding = resolved_by_source
            .get(&track.binding)
            .expect("every compiled source binding was resolved")
            .clone();
    }
    if let Some(runtime_time_base) = request.runtime_time_base {
        rebound = rebound.retimed(runtime_time_base).map_err(|error| {
            ClipInstantiationError::single(issue(
                ClipInstantiationCode::TimeBaseConversionFailed,
                "request.runtime_time_base",
                format!(
                    "source clock {} could not be normalized to runtime clock {}: {error}",
                    sequence.time_base.ticks_per_second, runtime_time_base.ticks_per_second
                ),
                "Use a non-zero runtime clock whose integer range can represent the complete source clip.",
            ))
        })?;
    }
    let plan = rebound.compile().map_err(|error| {
        ClipInstantiationError::new(
            error
                .issues
                .into_iter()
                .map(|diagnostic| {
                    issue(
                        ClipInstantiationCode::SequenceCompilationFailed,
                        format!("rebound_sequence.{}", diagnostic.location),
                        diagnostic.message,
                        diagnostic.remediation,
                    )
                })
                .collect(),
        )
    })?;
    let mapped_binding_signature = BindingSignature::with_element_counts(
        resolved.iter().map(|entry| entry.target.clone()),
        resolved
            .iter()
            .filter_map(|entry| {
                source_signature
                    .element_counts
                    .get(&entry.source)
                    .copied()
                    .map(|count| (entry.target.clone(), count))
            })
            .collect(),
    );
    let stable_hash = clip_instance_hash(
        &source,
        &source_signature,
        &mapped_binding_signature,
        &resolved,
        request,
        &plan,
    );
    Ok(CompiledClipInstance {
        instance_id: request.instance_id.clone(),
        source,
        source_binding_signature: source_signature,
        mapped_binding_signature,
        rebindings: resolved,
        stable_hash,
        plan,
    })
}

fn validated_mapping_tables<'a>(
    source: &BindingSignature,
    request: &'a ClipInstantiationRequest,
) -> (
    BTreeMap<&'a str, &'a String>,
    BTreeMap<&'a Binding, &'a Binding>,
    Vec<ClipInstantiationIssue>,
) {
    let source_targets: BTreeSet<_> = source
        .bindings
        .iter()
        .map(|binding| binding.path.target.as_str())
        .collect();
    let source_bindings: BTreeSet<_> = source.bindings.iter().collect();
    let mut target_map = BTreeMap::new();
    let mut exact_map = BTreeMap::new();
    let mut issues = Vec::new();

    for (index, rebind) in request.target_rebinds.iter().enumerate() {
        let location = format!("request.target_rebinds[{index}]");
        if rebind.source_target.trim().is_empty()
            || rebind.target_target.trim().is_empty()
            || rebind.source_target.contains(['/', '\0'])
            || rebind.target_target.contains(['/', '\0'])
        {
            issues.push(issue(
                ClipInstantiationCode::InvalidBinding,
                location,
                "target rebind contains an empty, slash-bearing, or NUL target segment",
                "Use non-empty stable source and target IDs.",
            ));
            continue;
        }
        if !source_targets.contains(rebind.source_target.as_str()) {
            issues.push(issue(
                ClipInstantiationCode::UnknownSourceTarget,
                format!("{location}.source_target"),
                format!(
                    "source target '{}' is not animated by this clip",
                    rebind.source_target
                ),
                "Remove the stale mapping or refresh it from the source binding signature.",
            ));
        }
        if target_map
            .insert(rebind.source_target.as_str(), &rebind.target_target)
            .is_some()
        {
            issues.push(issue(
                ClipInstantiationCode::DuplicateTargetRebind,
                format!("{location}.source_target"),
                format!(
                    "source target '{}' is mapped more than once",
                    rebind.source_target
                ),
                "Keep exactly one target-level mapping per source target.",
            ));
        }
    }

    for (index, rebind) in request.binding_rebinds.iter().enumerate() {
        let location = format!("request.binding_rebinds[{index}]");
        if !valid_binding(&rebind.source) || !valid_binding(&rebind.target) {
            issues.push(issue(
                ClipInstantiationCode::InvalidBinding,
                location.clone(),
                "full-path rebind contains an invalid source or target binding",
                "Use complete typed paths with non-empty slash-free segments.",
            ));
        }
        if !source_bindings.contains(&rebind.source) {
            issues.push(issue(
                ClipInstantiationCode::UnknownSourceBinding,
                format!("{location}.source"),
                format!(
                    "source binding '{}' ({:?}) is not animated by this clip",
                    rebind.source.path.display_path(),
                    rebind.source.value_kind
                ),
                "Remove the stale mapping or refresh it from the source binding signature.",
            ));
        }
        if rebind.source.value_kind != rebind.target.value_kind {
            issues.push(issue(
                ClipInstantiationCode::BindingTypeMismatch,
                format!("{location}.target.value_kind"),
                "full-path rebind changes the declared animation value kind",
                "Choose a target property with the same value kind as the source.",
            ));
        }
        if exact_map.insert(&rebind.source, &rebind.target).is_some() {
            issues.push(issue(
                ClipInstantiationCode::DuplicateBindingRebind,
                format!("{location}.source"),
                format!(
                    "source binding '{}' is mapped more than once",
                    rebind.source.path.display_path()
                ),
                "Keep exactly one full-path mapping per source binding.",
            ));
        }
    }
    (target_map, exact_map, issues)
}

fn validate_skeleton_contract(
    source: &ClipInstanceSource,
    source_bindings: &BindingSignature,
    request: &ClipInstantiationRequest,
) -> Vec<ClipInstantiationIssue> {
    let skeletal = source_bindings
        .bindings
        .iter()
        .any(|binding| binding.path.component == "SkeletonJoint");
    let mut issues = Vec::new();
    if let Some(signature) = &source.skeleton_signature {
        issues.extend(skeleton_signature_issues(
            signature,
            "source.skeleton_signature",
        ));
    }
    if let Some(signature) = &request.target_skeleton {
        issues.extend(skeleton_signature_issues(
            signature,
            "request.target_skeleton",
        ));
    }
    if !skeletal {
        return issues;
    }
    let Some(source_skeleton) = &source.skeleton_signature else {
        issues.push(issue(
            ClipInstantiationCode::MissingSkeletonSignature,
            "source.skeleton_signature",
            "skeletal clip has no stable source skeleton signature",
            "Re-import the clip with hierarchy and bind-pose signature generation enabled.",
        ));
        return issues;
    };
    let Some(target_skeleton) = &request.target_skeleton else {
        issues.push(issue(
            ClipInstantiationCode::MissingTargetSkeleton,
            "request.target_skeleton",
            "skeletal clip instancing requires an explicit target skeleton",
            "Select a target rig and provide its measured skeleton signature.",
        ));
        return issues;
    };
    if source_skeleton.signature_hash != target_skeleton.signature_hash {
        issues.push(issue(
            ClipInstantiationCode::IncompatibleSkeleton,
            "request.target_skeleton.signature_hash",
            format!(
                "source hierarchy '{}' does not match target hierarchy '{}'",
                source_skeleton.signature_hash, target_skeleton.signature_hash
            ),
            "Choose the exact source rig; cross-rig retargeting requires a separate verified retarget profile.",
        ));
    } else if request.require_rest_pose_match
        && source_skeleton.rest_pose_hash != target_skeleton.rest_pose_hash
    {
        issues.push(issue(
            ClipInstantiationCode::RestPoseMismatch,
            "request.target_skeleton.rest_pose_hash",
            format!(
                "source bind pose '{}' does not match target bind pose '{}'",
                source_skeleton.rest_pose_hash, target_skeleton.rest_pose_hash
            ),
            "Use a rig with the same bind pose or explicitly run a verified retargeting stage.",
        ));
    }
    issues
}

fn skeleton_signature_issues(
    signature: &SkeletonSignature,
    location: &str,
) -> Vec<ClipInstantiationIssue> {
    let mut issues = Vec::new();
    if signature.signature_hash.trim().is_empty()
        || signature.rest_pose_hash.trim().is_empty()
        || signature.joint_names.is_empty()
        || signature.joint_names.len() != signature.parent_indices.len()
    {
        issues.push(issue(
            ClipInstantiationCode::InvalidSkeletonSignature,
            location,
            "skeleton signature is missing hashes, joints, or aligned parent metadata",
            "Recompute the skeleton signature from the complete hierarchy and bind pose.",
        ));
        return issues;
    }
    let joint_count = signature.joint_names.len();
    if signature
        .joint_names
        .iter()
        .any(|name| name.trim().is_empty())
        || signature
            .parent_indices
            .iter()
            .enumerate()
            .any(|(index, parent)| {
                parent.is_some_and(|parent| {
                    usize::try_from(parent)
                        .map_or(true, |parent| parent >= joint_count || parent == index)
                })
            })
        || skeleton_has_cycle(&signature.parent_indices)
    {
        issues.push(issue(
            ClipInstantiationCode::InvalidSkeletonSignature,
            location,
            "skeleton signature contains an empty joint name, invalid parent, or parent cycle",
            "Rebuild a finite acyclic hierarchy with one parent index per joint.",
        ));
    }
    issues
}

fn skeleton_has_cycle(parents: &[Option<u32>]) -> bool {
    for start in 0..parents.len() {
        let mut visited = BTreeSet::new();
        let mut cursor = Some(start);
        while let Some(index) = cursor {
            if !visited.insert(index) {
                return true;
            }
            cursor = parents[index].and_then(|parent| usize::try_from(parent).ok());
        }
    }
    false
}

fn valid_binding(binding: &Binding) -> bool {
    [
        binding.path.target.as_str(),
        binding.path.component.as_str(),
        binding.path.property.as_str(),
    ]
    .into_iter()
    .chain(binding.path.subpath.iter().map(String::as_str))
    .all(|segment| !segment.trim().is_empty() && !segment.contains(['/', '\0']))
}

fn binding_signature_hash(bindings: &[Binding], element_counts: &BTreeMap<Binding, u32>) -> String {
    let mut hash = StableHasher::new();
    hash.u32(BINDING_SIGNATURE_VERSION);
    hash.u64(bindings.len() as u64);
    for binding in bindings {
        hash.string(&binding.path.target);
        hash.string(&binding.path.component);
        hash.string(&binding.path.property);
        hash.u64(binding.path.subpath.len() as u64);
        for segment in &binding.path.subpath {
            hash.string(segment);
        }
        hash.u8(value_kind_tag(binding.value_kind));
        match element_counts.get(binding) {
            Some(count) => {
                hash.u8(1);
                hash.u32(*count);
            }
            None => hash.u8(0),
        }
    }
    format!("mtkbind:{:032x}", hash.finish())
}

fn clip_instance_hash(
    source: &ClipInstanceSource,
    source_signature: &BindingSignature,
    mapped_signature: &BindingSignature,
    mappings: &[BindingRebind],
    request: &ClipInstantiationRequest,
    plan: &CompiledSequence,
) -> String {
    let mut hash = StableHasher::new();
    hash.string(&request.instance_id);
    hash.string(source.logical_asset_id.as_str());
    hash.string(source.revision_id.as_str());
    hash.string(&source.content_hash);
    hash.string(source.clip_id.as_str());
    hash.string(source.sequence_id.as_str());
    hash.string(&source_signature.signature_hash);
    hash.string(&mapped_signature.signature_hash);
    hash.u64(mappings.len() as u64);
    for mapping in mappings {
        hash.string(&mapping.source.path.display_path());
        hash.u8(value_kind_tag(mapping.source.value_kind));
        hash.string(&mapping.target.path.display_path());
        hash.u8(value_kind_tag(mapping.target.value_kind));
    }
    match request.target_skeleton.as_ref() {
        Some(skeleton) => {
            hash.u8(1);
            hash.string(&skeleton.signature_hash);
            hash.string(&skeleton.rest_pose_hash);
        }
        None => hash.u8(0),
    }
    hash.u8(u8::from(request.require_rest_pose_match));
    hash.string(&plan.stable_hash);
    format!("mtkclipinst:{:032x}", hash.finish())
}

fn issue(
    code: ClipInstantiationCode,
    location: impl Into<String>,
    message: impl Into<String>,
    remediation: impl Into<String>,
) -> ClipInstantiationIssue {
    ClipInstantiationIssue::new(code, location, message, remediation)
}

const fn value_kind_tag(kind: ValueKind) -> u8 {
    match kind {
        ValueKind::Number => 0,
        ValueKind::Integer => 1,
        ValueKind::Boolean => 2,
        ValueKind::String => 3,
        ValueKind::Vec3 => 4,
        ValueKind::Vec4 => 5,
        ValueKind::Quaternion => 6,
        ValueKind::Weights => 7,
    }
}

struct StableHasher(u128);

impl StableHasher {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

    const fn new() -> Self {
        Self(Self::OFFSET)
    }

    const fn finish(self) -> u128 {
        self.0
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u128::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn u8(&mut self, value: u8) {
        self.bytes(&[value]);
    }

    fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn string(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.bytes(value.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnimValue, Interpolation, KeyId, Keyframe, PropertyPath, Tick, Track, TrackId};

    fn binding(target: &str, component: &str, property: &str, kind: ValueKind) -> Binding {
        Binding {
            path: PropertyPath::new(target, component, property),
            value_kind: kind,
        }
    }

    fn sequence() -> Sequence {
        let mut sequence = Sequence::new("source:walk", "Walk", Tick(100));
        sequence.tracks = vec![
            Track {
                id: TrackId::new("track:rotation"),
                name: "rotation".into(),
                binding: binding(
                    "gltf-node:1",
                    "SkeletonJoint",
                    "rotation",
                    ValueKind::Quaternion,
                ),
                interpolation: Interpolation::Linear,
                keyframes: vec![Keyframe::new(
                    KeyId::new("key:rotation"),
                    Tick::ZERO,
                    AnimValue::Quaternion([0.0, 0.0, 0.0, 1.0]),
                )],
                enabled: true,
            },
            Track {
                id: TrackId::new("track:translation"),
                name: "translation".into(),
                binding: binding(
                    "gltf-node:1",
                    "SkeletonJoint",
                    "translation",
                    ValueKind::Vec3,
                ),
                interpolation: Interpolation::Linear,
                keyframes: vec![Keyframe::new(
                    KeyId::new("key:translation"),
                    Tick::ZERO,
                    AnimValue::Vec3([0.0; 3]),
                )],
                enabled: true,
            },
        ];
        sequence
    }

    fn skeleton(hash: &str, rest: &str) -> SkeletonSignature {
        SkeletonSignature {
            signature_hash: hash.into(),
            rest_pose_hash: rest.into(),
            joint_names: vec!["hips".into()],
            parent_indices: vec![None],
            humanoid_profile: None,
        }
    }

    fn source() -> ClipInstanceSource {
        ClipInstanceSource {
            logical_asset_id: AnimationAssetId::new("asset:walk"),
            revision_id: AnimationRevisionId::new("revision:1"),
            content_hash: "content:1".into(),
            clip_id: ClipId::new("clip:walk"),
            sequence_id: SequenceId::new("source:walk"),
            skeleton_signature: Some(skeleton("rig:1", "rest:1")),
        }
    }

    #[test]
    fn binding_signature_is_order_independent_and_detects_tampering() {
        let first = binding("a", "Transform", "translation", ValueKind::Vec3);
        let second = binding("b", "Transform", "rotation", ValueKind::Quaternion);
        let left = BindingSignature::new([first.clone(), second.clone(), first]);
        let right = BindingSignature::new([second, left.bindings[0].clone()]);
        assert_eq!(left, right);

        let mut tampered = left;
        tampered.signature_hash = "stale".into();
        assert_eq!(
            tampered.validation_issues("catalog")[0].code,
            ClipInstantiationCode::BindingSignatureHashMismatch
        );
    }

    #[test]
    fn explicit_full_path_rebind_builds_a_playable_immutable_instance() {
        let source_sequence = sequence();
        let source_signature = BindingSignature::from_sequence(&source_sequence);
        let target_rotation = binding(
            "entity:hero",
            "Transform",
            "rotation",
            ValueKind::Quaternion,
        );
        let target_translation =
            binding("entity:hero", "Transform", "translation", ValueKind::Vec3);
        let mut request = ClipInstantiationRequest::new(
            "instance:hero-walk",
            ClipId::new("clip:walk"),
            BindingSignature::new([target_rotation.clone(), target_translation.clone()]),
        );
        request.expected_source_binding_hash = Some(source_signature.signature_hash);
        request.target_skeleton = Some(skeleton("rig:1", "rest:1"));
        request.binding_rebinds = vec![
            BindingRebind::new(
                binding(
                    "gltf-node:1",
                    "SkeletonJoint",
                    "rotation",
                    ValueKind::Quaternion,
                ),
                target_rotation.clone(),
            ),
            BindingRebind::new(
                binding(
                    "gltf-node:1",
                    "SkeletonJoint",
                    "translation",
                    ValueKind::Vec3,
                ),
                target_translation.clone(),
            ),
        ];

        let instance =
            instantiate_clip(source(), &source_sequence, &request).expect("valid instance");
        assert_eq!(instance.plan.tracks_len(), 2);
        let paths: BTreeSet<_> = instance
            .plan
            .evaluate(Tick::ZERO)
            .bindings
            .into_iter()
            .map(|sample| sample.binding)
            .collect();
        assert_eq!(paths, BTreeSet::from([target_rotation, target_translation]));
        assert_eq!(
            source_sequence.tracks[0].binding.path.target, "gltf-node:1",
            "instancing must not mutate imported source data"
        );
        request.binding_rebinds.reverse();
        let reordered = instantiate_clip(source(), &source_sequence, &request).unwrap();
        assert_eq!(
            instance.stable_hash, reordered.stable_hash,
            "mapping arrival order cannot change the compile fence"
        );

        let mut nanosecond_source = source_sequence.clone();
        nanosecond_source.time_base = crate::TimeBase::new(1_000_000_000);
        nanosecond_source.duration = Tick(2_000_000_000);
        request.runtime_time_base = Some(crate::TimeBase::GAME_60);
        let normalized = instantiate_clip(source(), &nanosecond_source, &request)
            .expect("the admitted plan normalizes onto the graph clock");
        assert_eq!(normalized.plan.time_base, crate::TimeBase::GAME_60);
        assert_eq!(normalized.plan.duration, Tick(120_000));
        assert_ne!(
            normalized.stable_hash, instance.stable_hash,
            "the runtime clock is part of the immutable plan fence"
        );
    }

    #[test]
    fn target_level_rebind_is_expanded_and_canonical() {
        let source_sequence = sequence();
        let targets = BindingSignature::new([
            binding(
                "rig:hero:hips",
                "SkeletonJoint",
                "rotation",
                ValueKind::Quaternion,
            ),
            binding(
                "rig:hero:hips",
                "SkeletonJoint",
                "translation",
                ValueKind::Vec3,
            ),
        ]);
        let mut request =
            ClipInstantiationRequest::new("instance:target-map", ClipId::new("clip:walk"), targets);
        request.target_skeleton = Some(skeleton("rig:1", "rest:1"));
        request.target_rebinds = vec![TargetRebind::new("gltf-node:1", "rig:hero:hips")];
        let instance = instantiate_clip(source(), &source_sequence, &request).unwrap();
        assert_eq!(instance.rebindings.len(), 2);
        assert!(instance
            .rebindings
            .iter()
            .all(|mapping| mapping.target.path.target == "rig:hero:hips"));
    }

    #[test]
    fn stale_unknown_and_many_to_one_mappings_fail_deterministically() {
        let source_sequence = sequence();
        let target = binding("entity:hero", "Transform", "value", ValueKind::Vec3);
        let mut request = ClipInstantiationRequest::new(
            "instance:bad",
            ClipId::new("clip:walk"),
            BindingSignature::new([target.clone()]),
        );
        request.target_skeleton = Some(skeleton("rig:1", "rest:1"));
        request.binding_rebinds = vec![
            BindingRebind::new(
                binding(
                    "gltf-node:missing",
                    "SkeletonJoint",
                    "translation",
                    ValueKind::Vec3,
                ),
                target.clone(),
            ),
            BindingRebind::new(
                binding(
                    "gltf-node:1",
                    "SkeletonJoint",
                    "translation",
                    ValueKind::Vec3,
                ),
                target,
            ),
        ];
        let error = instantiate_clip(source(), &source_sequence, &request).unwrap_err();
        let codes: BTreeSet<_> = error.issues.iter().map(|item| item.code).collect();
        assert!(codes.contains(&ClipInstantiationCode::UnknownSourceBinding));
        assert!(codes.contains(&ClipInstantiationCode::UnmappedSourceBinding));
        assert_eq!(
            error.issues,
            ClipInstantiationError::new(error.issues.clone()).issues,
            "diagnostics are already in stable location/code order"
        );
    }

    #[test]
    fn skeletal_instance_requires_exact_hierarchy_and_rest_pose() {
        let source_sequence = sequence();
        let source_bindings = BindingSignature::from_sequence(&source_sequence);
        let mut request = ClipInstantiationRequest::new(
            "instance:rig",
            ClipId::new("clip:walk"),
            source_bindings,
        );
        request.unmapped_policy = UnmappedBindingPolicy::KeepSource;
        request.target_skeleton = Some(skeleton("rig:other", "rest:other"));
        let error = instantiate_clip(source(), &source_sequence, &request).unwrap_err();
        assert!(error
            .issues
            .iter()
            .any(|item| item.code == ClipInstantiationCode::IncompatibleSkeleton));

        request.target_skeleton = Some(skeleton("rig:1", "rest:other"));
        let error = instantiate_clip(source(), &source_sequence, &request).unwrap_err();
        assert!(error
            .issues
            .iter()
            .any(|item| item.code == ClipInstantiationCode::RestPoseMismatch));

        request.target_skeleton = Some(skeleton("rig:1", "rest:1"));
        let exact = instantiate_clip(source(), &source_sequence, &request).unwrap();
        request.target_skeleton = Some(skeleton("rig:1", "rest:other"));
        request.require_rest_pose_match = false;
        let relaxed = instantiate_clip(source(), &source_sequence, &request).unwrap();
        assert_ne!(
            exact.stable_hash, relaxed.stable_hash,
            "target bind pose and rest-match policy are part of the compile fence"
        );
    }
}
