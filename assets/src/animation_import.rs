//! glTF animation ingestion into `metrocalk-animation`.
//!
//! This is deliberately a separate vertical from [`crate::mesh::MeshAsset`]: a file can contain motion
//! without drawable geometry, and mesh-only consumers should not pay for or accidentally own timeline
//! state. `gltf::` values stay private to this wrapper; the public result contains only project types.

#![allow(clippy::cast_precision_loss, clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::mem::size_of;

use gltf::animation::util::ReadOutputs;
use gltf::animation::{Interpolation as GltfInterpolation, Property};
use gltf::Gltf;
use metrocalk_animation::{
    instantiate_clip, AnimValue, AnimationAssetId, AnimationAssetManifest, AnimationAssetRecord,
    AnimationCapability, AnimationEditorContext, AnimationEditorRepresentation, AnimationFacet,
    AnimationFacetId, AnimationImportState, AnimationImportStatus, AnimationReimportDiagnostic,
    Binding, BindingSignature, ClipId, ClipInstanceSource, ClipInstantiationCode,
    ClipInstantiationError, ClipInstantiationIssue, ClipInstantiationRequest, ClipSummary,
    CompiledClipInstance, CompressionMetadata, ContactMetadata, ContentIdentity,
    DeterminismMetadata, EventMetadata, ExtensionMetadata, Interpolation, KeyId, Keyframe,
    MorphMetadata, PlaybackTarget, PropertyPath, ReimportMetadata, RootMotionMetadata, Sequence,
    SequenceId, Severity, SkeletonCompatibility, SkeletonMatchPolicy, SkeletonSignature,
    SourceIdentity, Tick, TimeBase, Track, TrackId, ValueKind, ANIMATION_MANIFEST_VERSION,
};
use serde::{Deserialize, Deserializer, Serialize};

use crate::gltf_import::GltfImporter;
use crate::source::{ImportError, MAX_ELEMENTS, MAX_IMPORT_BYTES};

/// Nanosecond authored clock. This retains every normally-authored f32 glTF time without introducing a
/// floating runtime clock; pathological sub-nanosecond collisions remain visible as a quality fact.
pub const GLTF_ANIMATION_TIME_BASE: TimeBase = TimeBase::new(1_000_000_000);

/// A stable target discovered in the source hierarchy. `id` hashes the canonical named hierarchy path,
/// while `source_node_index` retains format-local provenance. The ID remains the binding address whether
/// or not that node also participates in a skin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationTargetIdentity {
    pub id: String,
    pub source_node_index: u32,
    pub display_name: String,
    /// Canonical hierarchy path (name plus same-name sibling ordinal per segment). Unlike a source node
    /// index, this remains stable when unrelated glTF nodes are inserted or reordered.
    #[serde(default)]
    pub canonical_path: String,
    /// `(skin index, joint slot)` memberships; empty for ordinary scene nodes.
    pub joint_memberships: Vec<(u32, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationImportCode {
    NoAnimations,
    EmptyAnimation,
    UnresolvedBuffer,
    EmptyInput,
    NonFiniteTime,
    NegativeTime,
    NonMonotonicTime,
    QuantizedTimeCollision,
    OutputCardinalityMismatch,
    NonFiniteOutput,
    InvalidQuaternion,
    UnknownMorphTargetCount,
    ElementLimitExceeded,
    KernelValidationFailed,
    MultipleSkeletons,
    CrossSkeletonClip,
    SurplusInverseBindMatrices,
    UnsupportedAnimationPointer,
    StaticDurationPadded,
}

/// Non-fatal or channel-local ingestion outcome. A broken channel never silently disappears: it is skipped
/// with a stable code, precise source location, plain-language impact, and remediation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationImportFact {
    pub severity: Severity,
    pub code: AnimationImportCode,
    pub location: String,
    pub message: String,
    pub remediation: String,
}

/// Animation-only product of one glTF import.
///
/// `manifest` remains directly available for existing playback callers. `record` is the repository-facing
/// lifecycle wrapper around the same immutable manifest; importer-created values keep the two copies equal.
/// Deserialization accepts the earlier four-field JSON shape and derives the missing record from its
/// manifest and retained import facts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AnimationAsset {
    pub sequences: Vec<Sequence>,
    pub manifest: AnimationAssetManifest,
    pub record: AnimationAssetRecord,
    pub targets: Vec<AnimationTargetIdentity>,
    pub facts: Vec<AnimationImportFact>,
}

#[derive(Deserialize)]
struct AnimationAssetWire {
    sequences: Vec<Sequence>,
    manifest: AnimationAssetManifest,
    #[serde(default)]
    record: Option<AnimationAssetRecord>,
    targets: Vec<AnimationTargetIdentity>,
    facts: Vec<AnimationImportFact>,
}

impl<'de> Deserialize<'de> for AnimationAsset {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = AnimationAssetWire::deserialize(deserializer)?;
        let record = wire
            .record
            .unwrap_or_else(|| imported_asset_record(&wire.manifest, &wire.facts));
        if record.manifest != wire.manifest {
            return Err(serde::de::Error::custom(
                "animation record manifest does not match the legacy manifest field",
            ));
        }
        Ok(Self {
            sequences: wire.sequences,
            manifest: wire.manifest,
            record,
            targets: wire.targets,
            facts: wire.facts,
        })
    }
}

impl AnimationAsset {
    #[must_use]
    pub fn can_play_on(&self, target: &PlaybackTarget) -> bool {
        self.is_ready_for_playback() && self.record.can_play_on(target)
    }

    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.record.has_fatal_readiness_error()
    }

    /// Fatal readiness includes both the importer outcome and the immutable manifest contract. A file with
    /// only warnings can retain a playable partial clip; a fatal skipped channel or a structurally invalid
    /// manifest must never be advertised as ready merely because the target supports its capabilities.
    #[must_use]
    pub fn is_ready_for_playback(&self) -> bool {
        self.sequences
            .iter()
            .any(|sequence| !sequence.tracks.is_empty())
            && matches!(
                self.record.import_status.state,
                AnimationImportState::Ready | AnimationImportState::ReadyWithWarnings
            )
            && !self.record.has_fatal_readiness_error()
    }

    /// Canonical typed source-binding signature for one imported clip.
    ///
    /// This remains available for mapping/repair UX even when the asset is not currently playable.
    pub fn clip_binding_signature(
        &self,
        clip_id: &ClipId,
    ) -> Result<BindingSignature, ClipInstantiationError> {
        let (_, sequence, _) = self.resolve_clip(clip_id)?;
        Ok(BindingSignature::from_sequence(sequence))
    }

    /// Resolve, explicitly rebind and compile one imported clip without mutating the cooked asset.
    ///
    /// The request's target catalog is authoritative. Every emitted binding must resolve into it, and
    /// skeletal tracks additionally require a measured compatible hierarchy/bind-pose signature.
    pub fn instantiate_clip(
        &self,
        request: &ClipInstantiationRequest,
    ) -> Result<CompiledClipInstance, ClipInstantiationError> {
        if !self.is_ready_for_playback() {
            return Err(ClipInstantiationError::single(asset_issue(
                ClipInstantiationCode::AssetNotReady,
                "asset.record.import_status",
                format!(
                    "animation asset '{}' is not in a playable validated state",
                    self.record.effective_logical_id().as_str()
                ),
                "Repair import/manifest diagnostics and retain a successful cooked revision before instancing.",
            )));
        }
        if self.record.manifest != self.manifest {
            return Err(ClipInstantiationError::single(asset_issue(
                ClipInstantiationCode::AssetContractMismatch,
                "asset.record.manifest",
                "repository record manifest diverges from the animation asset manifest",
                "Reload the authoritative cooked revision; do not combine metadata from different revisions.",
            )));
        }
        let (facet, sequence, summary) = self.resolve_clip(&request.clip_id)?;
        let enabled_track_count = sequence
            .tracks
            .iter()
            .filter(|track| track.enabled && !track.keyframes.is_empty())
            .count();
        let summary_matches = summary.sequence_id == sequence.id
            && summary.duration == sequence.duration
            && summary.time_base == sequence.time_base
            && usize::try_from(summary.track_count).ok() == Some(sequence.tracks.len())
            && enabled_track_count > 0;
        if !summary_matches {
            return Err(ClipInstantiationError::single(asset_issue(
                ClipInstantiationCode::AssetContractMismatch,
                format!("manifest.clips[{}]", request.clip_id),
                "clip summary does not match its authoritative imported sequence",
                "Re-import the asset so clip identity, timing and track counts are generated atomically.",
            )));
        }
        let binding_signature = BindingSignature::from_sequence(sequence);
        let skeletal = binding_signature
            .bindings
            .iter()
            .any(|binding| binding.path.component == "SkeletonJoint");
        let source = ClipInstanceSource {
            logical_asset_id: self.record.effective_logical_id().clone(),
            revision_id: self.record.effective_revision_id(),
            content_hash: self.manifest.content.content_hash.clone(),
            clip_id: summary.clip_id.clone(),
            sequence_id: summary.sequence_id.clone(),
            skeleton_signature: skeletal.then(|| facet.skeleton_signature.clone()).flatten(),
        };
        instantiate_clip(source, sequence, request)
    }

    fn resolve_clip<'a>(
        &'a self,
        clip_id: &ClipId,
    ) -> Result<(&'a AnimationFacet, &'a Sequence, &'a ClipSummary), ClipInstantiationError> {
        let matches: Vec<_> = self
            .manifest
            .facets
            .iter()
            .flat_map(|facet| {
                facet
                    .clips
                    .iter()
                    .filter(move |summary| summary.clip_id == *clip_id)
                    .map(move |summary| (facet, summary))
            })
            .collect();
        let (facet, summary) = match matches.as_slice() {
            [] => {
                return Err(ClipInstantiationError::single(asset_issue(
                    ClipInstantiationCode::MissingClip,
                    "manifest.facets.clips",
                    format!("clip '{clip_id}' does not exist in this animation asset"),
                    "Refresh the clip list and select a clip from the current cooked revision.",
                )));
            }
            [resolved] => *resolved,
            _ => {
                return Err(ClipInstantiationError::single(asset_issue(
                    ClipInstantiationCode::AmbiguousClip,
                    "manifest.facets.clips",
                    format!(
                        "clip ID '{}' is duplicated across {} animation facets",
                        clip_id,
                        matches.len()
                    ),
                    "Assign globally unique clip IDs within the animation asset.",
                )));
            }
        };
        let sequences: Vec<_> = self
            .sequences
            .iter()
            .filter(|sequence| sequence.id == summary.sequence_id)
            .collect();
        let sequence = match sequences.as_slice() {
            [] => {
                return Err(ClipInstantiationError::single(asset_issue(
                    ClipInstantiationCode::MissingSequence,
                    format!("manifest.clips[{clip_id}].sequence_id"),
                    format!(
                        "clip '{}' references missing sequence '{}'",
                        clip_id, summary.sequence_id
                    ),
                    "Re-import the asset so every clip summary references a retained sequence.",
                )));
            }
            [sequence] => *sequence,
            _ => {
                return Err(ClipInstantiationError::single(asset_issue(
                    ClipInstantiationCode::AmbiguousSequence,
                    "asset.sequences",
                    format!(
                        "sequence ID '{}' occurs {} times",
                        summary.sequence_id,
                        sequences.len()
                    ),
                    "Assign a unique stable ID to every imported sequence.",
                )));
            }
        };
        Ok((facet, sequence, summary))
    }
}

fn asset_issue(
    code: ClipInstantiationCode,
    location: impl Into<String>,
    message: impl Into<String>,
    remediation: impl Into<String>,
) -> ClipInstantiationIssue {
    ClipInstantiationIssue::new(code, location, message, remediation)
}

fn empty_root_motion() -> RootMotionMetadata {
    RootMotionMetadata {
        present: false,
        source_joint: None,
        translation_axes: [false; 3],
        extracts_yaw: false,
        average_speed_metres_per_second: 0.0,
        in_place_preview_supported: false,
    }
}

fn empty_event_metadata() -> EventMetadata {
    EventMetadata {
        names: BTreeSet::new(),
        payload_kinds: BTreeSet::new(),
        interval_crossing_safe: true,
    }
}

impl GltfImporter {
    /// Import animation clips without requiring drawable mesh geometry.
    pub fn import_animations(&self, bytes: &[u8]) -> Result<AnimationAsset, ImportError> {
        let source_hash = content_hash(bytes);
        self.import_animations_with_logical_id(
            bytes,
            "memory://gltf",
            AnimationAssetId::new(format!("animation:{source_hash}")),
        )
    }

    /// Import animation clips while retaining a caller-provided source URI for re-import UX.
    ///
    /// This standalone convenience remains content-identified: a URI is evidence, never logical identity.
    /// Repositories must call [`Self::import_animations_with_logical_id`] with their persistent asset ID.
    pub fn import_animations_with_source(
        &self,
        bytes: &[u8],
        source_uri: &str,
    ) -> Result<AnimationAsset, ImportError> {
        let logical_asset_id = AnimationAssetId::new(format!("animation:{}", content_hash(bytes)));
        self.import_animations_with_logical_id(bytes, source_uri, logical_asset_id)
    }

    /// Import a new cooked revision beneath a caller-owned stable logical asset identity.
    pub fn import_animations_with_logical_id(
        &self,
        bytes: &[u8],
        source_uri: &str,
        logical_asset_id: AnimationAssetId,
    ) -> Result<AnimationAsset, ImportError> {
        if source_uri.trim().is_empty() {
            return Err(ImportError::Malformed(
                "animation source URI must be non-empty".into(),
            ));
        }
        if logical_asset_id.as_str().trim().is_empty() {
            return Err(ImportError::Malformed(
                "animation logical asset ID must be non-empty".into(),
            ));
        }
        if bytes.len() > MAX_IMPORT_BYTES {
            return Err(ImportError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_IMPORT_BYTES,
            });
        }
        let doc =
            Gltf::from_slice(bytes).map_err(|error| ImportError::Malformed(error.to_string()))?;
        let blob = doc.blob.as_deref().unwrap_or(&[]);
        guard_document_count(doc.nodes().count())?;
        guard_document_count(doc.skins().count())?;
        guard_document_count(doc.animations().count())?;
        let hierarchy = ValidatedNodeHierarchy::from_document(&doc)?;
        let canonical_paths = canonical_node_paths(&doc, &hierarchy)?;
        validate_skin_joint_tables(&doc)?;
        let source_hash = content_hash(bytes);
        let targets = target_identities(&doc, &canonical_paths)?;
        let target_ids: HashMap<_, _> = targets
            .iter()
            .map(|target| {
                (
                    usize::try_from(target.source_node_index)
                        .expect("guarded glTF node index fits usize"),
                    target.id.clone(),
                )
            })
            .collect();
        let joint_nodes: BTreeSet<usize> = doc
            .skins()
            .flat_map(|skin| skin.joints().map(|node| node.index()))
            .collect();
        let mut skin_memberships_by_node: HashMap<usize, BTreeSet<usize>> = HashMap::new();
        for skin in doc.skins() {
            for node in skin.joints() {
                skin_memberships_by_node
                    .entry(node.index())
                    .or_default()
                    .insert(skin.index());
            }
        }
        let local_matrices: Vec<_> = doc.nodes().map(|node| node.transform().matrix()).collect();
        let mut facts = inverse_bind_matrix_surplus_facts(&doc);
        let mut skeleton_budget = SkeletonAdmissionBudget::default();
        let skeleton_signatures = build_skeleton_signatures(
            &doc,
            blob,
            &hierarchy,
            &canonical_paths,
            &local_matrices,
            &mut skeleton_budget,
        )?;
        let pointer_used = contains_ascii(bytes, b"KHR_animation_pointer");
        let pointer_required = extension_required_ascii(bytes, b"KHR_animation_pointer");
        if pointer_used {
            facts.push(fact(
                if pointer_required {
                    Severity::Error
                } else {
                    Severity::Warning
                },
                AnimationImportCode::UnsupportedAnimationPointer,
                "extensions.KHR_animation_pointer",
                "KHR_animation_pointer is declared but this ingestion tier supports core node TRS and morph channels only",
                "Convert pointer channels to core channels or install a playback target that implements the retained extension metadata.",
            ));
        }

        let mut sequences = Vec::new();
        let mut clip_profiles = Vec::new();
        let mut compiled_hashes = Vec::new();

        for animation in doc.animations() {
            let animation_index = animation.index();
            guard_document_count(animation.channels().count())?;
            let name = animation
                .name()
                .map_or_else(|| format!("Animation {animation_index}"), str::to_owned);
            let mut sequence = Sequence::new(
                SequenceId::new(format!("gltf-animation:{source_hash}:{animation_index}")),
                name.clone(),
                Tick(1),
            );
            sequence.time_base = GLTF_ANIMATION_TIME_BASE;
            let mut maximum_tick = Tick::ZERO;
            let mut capabilities = BTreeSet::from([AnimationCapability::ReversePlayback]);
            let mut maximum_morph_targets = 0usize;
            let mut channel_bindings = BTreeMap::new();
            let mut clip_skin_indices = BTreeSet::new();

            for channel in animation.channels() {
                let channel_index = channel.index();
                let source_location =
                    format!("animations[{animation_index}].channels[{channel_index}]");
                match decode_channel(
                    &channel,
                    blob,
                    &joint_nodes,
                    &target_ids,
                    animation_index,
                    &mut facts,
                ) {
                    Ok(decoded) => {
                        maximum_tick = maximum_tick.max(decoded.maximum_tick);
                        maximum_morph_targets =
                            maximum_morph_targets.max(decoded.morph_target_count);
                        if decoded.track.binding.value_kind != ValueKind::Weights {
                            capabilities.insert(AnimationCapability::TransformTracks);
                        }
                        if joint_nodes.contains(&channel.target().node().index()) {
                            capabilities.insert(AnimationCapability::SkeletalPose);
                            if let Some(memberships) =
                                skin_memberships_by_node.get(&channel.target().node().index())
                            {
                                clip_skin_indices.extend(memberships);
                            }
                        }
                        if decoded.track.binding.value_kind == ValueKind::Weights {
                            capabilities.insert(AnimationCapability::MorphTargets);
                        }
                        if decoded.track.interpolation == Interpolation::Cubic {
                            capabilities.insert(AnimationCapability::CubicInterpolation);
                        }
                        channel_bindings.insert(
                            format!("gltf:{animation_index}:{channel_index}"),
                            decoded.track.id.0.clone(),
                        );
                        sequence.tracks.push(decoded.track);
                    }
                    Err(failure) => facts.push(fact(
                        Severity::Error,
                        failure.code,
                        &source_location,
                        failure.message,
                        failure.remediation,
                    )),
                }
            }
            if sequence.tracks.is_empty() {
                facts.push(fact(
                    Severity::Warning,
                    AnimationImportCode::EmptyAnimation,
                    &format!("animations[{animation_index}]"),
                    "animation contains no decodable core channels",
                    "Inspect channel facts, external buffers, and extension requirements.",
                ));
            }
            if maximum_tick == Tick::ZERO {
                facts.push(fact(
                    Severity::Info,
                    AnimationImportCode::StaticDurationPadded,
                    &format!("animations[{animation_index}].duration"),
                    "zero-duration animation uses one nanosecond internally so it remains a valid sequence",
                    "No action is required; the clip summary retains the effective one-tick duration.",
                ));
                maximum_tick = Tick(1);
            }
            sequence.duration = maximum_tick;
            let compiled_hash = match sequence.compile() {
                Ok(compiled) => compiled.stable_hash,
                Err(error) => {
                    for issue in error.issues {
                        facts.push(fact(
                            issue.severity,
                            AnimationImportCode::KernelValidationFailed,
                            &format!("animations[{animation_index}].{}", issue.location),
                            issue.message,
                            issue.remediation,
                        ));
                    }
                    String::new()
                }
            };
            compiled_hashes.push(compiled_hash.clone());
            if clip_skin_indices.len() > 1 {
                facts.push(fact(
                    Severity::Error,
                    AnimationImportCode::CrossSkeletonClip,
                    &format!("animations[{animation_index}].channels"),
                    format!(
                        "clip targets joints from {} skins and has no single skeleton authority",
                        clip_skin_indices.len()
                    ),
                    "Split the motion into one clip per rig, or bake an explicit verified retargeted skeleton.",
                ));
            }
            let summary = ClipSummary {
                clip_id: ClipId::new(format!("gltf-clip:{source_hash}:{animation_index}")),
                sequence_id: sequence.id.clone(),
                name,
                duration: sequence.duration,
                time_base: sequence.time_base,
                track_count: u32::try_from(sequence.tracks.len())
                    .expect("channel count was guarded by MAX_ELEMENTS"),
                event_count: 0,
                marker_count: 0,
                animated_value_kinds: sequence
                    .tracks
                    .iter()
                    .map(|track| track.binding.value_kind)
                    .collect(),
            };
            clip_profiles.push(ImportedClipProfile {
                animation_index,
                summary,
                capabilities,
                maximum_morph_targets,
                skin_index: (clip_skin_indices.len() == 1)
                    .then(|| *clip_skin_indices.first().expect("single skin")),
                channel_bindings,
                compiled_hash,
            });
            sequences.push(sequence);
        }

        if sequences.is_empty() {
            facts.push(fact(
                Severity::Warning,
                AnimationImportCode::NoAnimations,
                "animations",
                "glTF contains no animation clips",
                "Choose a source with animation data or author a sequence in the editor.",
            ));
        }
        if doc.skins().count() > 1 {
            facts.push(fact(
                Severity::Warning,
                AnimationImportCode::MultipleSkeletons,
                "skins",
                "multiple skins are present; every clip is classified against its actual animated joint memberships",
                "Review any cross-skeleton clip error; otherwise each clip retains its own exact rig signature.",
            ));
        }

        let extensions = pointer_used
            .then(|| ExtensionMetadata {
                namespace: "KHR_animation_pointer".into(),
                version: 1,
                required: pointer_required,
                payload_hash: source_hash.clone(),
            })
            .into_iter()
            .collect::<Vec<_>>();
        let mut animation_facets: Vec<_> = clip_profiles
            .into_iter()
            .map(|profile| {
                let skeleton_signature = profile
                    .skin_index
                    .and_then(|index| skeleton_signatures.get(&index))
                    .cloned();
                let skeleton_compatibility =
                    skeleton_signature
                        .as_ref()
                        .map(|signature| SkeletonCompatibility {
                            policy: SkeletonMatchPolicy::Exact,
                            required_signature_hash: signature.signature_hash.clone(),
                            required_joints: signature.joint_names.iter().cloned().collect(),
                            required_humanoid_profile: None,
                            retarget_profile: None,
                        });
                AnimationFacet {
                    id: AnimationFacetId::new(format!(
                        "facet:gltf-animation:{}",
                        profile.animation_index
                    )),
                    capabilities: profile.capabilities,
                    clips: vec![profile.summary],
                    skeleton_signature,
                    skeleton_compatibility,
                    root_motion: empty_root_motion(),
                    contacts: Vec::<ContactMetadata>::new(),
                    events: empty_event_metadata(),
                    morphs: (profile.maximum_morph_targets > 0).then(|| MorphMetadata {
                        target_count: u32::try_from(profile.maximum_morph_targets)
                            .expect("morph count was guarded by MAX_ELEMENTS"),
                        target_names: (0..profile.maximum_morph_targets)
                            .map(|index| format!("morph_{index}"))
                            .collect(),
                        minimum_weight: 0.0,
                        maximum_weight: 1.0,
                    }),
                    extensions: extensions.clone(),
                    compression: None::<CompressionMetadata>,
                    reimport: ReimportMetadata {
                        strategy: "canonical-hierarchy-target-identity; channel-order identity remains revision-fenced".into(),
                        previous_source_hash: None,
                        preserves_authored_overrides: false,
                        channel_bindings: profile.channel_bindings,
                        orphaned_bindings: BTreeSet::new(),
                    },
                    determinism: DeterminismMetadata {
                        compiler_version: format!(
                            "metrocalk-assets/{}+metrocalk-animation/1",
                            env!("CARGO_PKG_VERSION")
                        ),
                        canonicalization_version: 2,
                        compiled_hash: if profile.compiled_hash.is_empty() {
                            content_hash(&[])
                        } else {
                            profile.compiled_hash
                        },
                        integer_tick_time: true,
                        stable_ordering: true,
                        deterministic_cpu_evaluation: true,
                    },
                }
            })
            .collect();
        if animation_facets.is_empty() {
            animation_facets.push(AnimationFacet {
                id: AnimationFacetId::new("facet:gltf-animation:empty"),
                capabilities: BTreeSet::from([AnimationCapability::ReversePlayback]),
                clips: Vec::new(),
                skeleton_signature: None,
                skeleton_compatibility: None,
                root_motion: empty_root_motion(),
                contacts: Vec::new(),
                events: empty_event_metadata(),
                morphs: None,
                extensions,
                compression: None,
                reimport: ReimportMetadata {
                    strategy: "canonical-hierarchy-target-identity".into(),
                    previous_source_hash: None,
                    preserves_authored_overrides: false,
                    channel_bindings: BTreeMap::new(),
                    orphaned_bindings: BTreeSet::new(),
                },
                determinism: DeterminismMetadata {
                    compiler_version: format!(
                        "metrocalk-assets/{}+metrocalk-animation/1",
                        env!("CARGO_PKG_VERSION")
                    ),
                    canonicalization_version: 2,
                    compiled_hash: content_hash(&[]),
                    integer_tick_time: true,
                    stable_ordering: true,
                    deterministic_cpu_evaluation: true,
                },
            });
        }
        let manifest = AnimationAssetManifest {
            schema_version: ANIMATION_MANIFEST_VERSION,
            asset_id: logical_asset_id,
            display_name: sequences
                .first()
                .map_or_else(|| "glTF Animation".into(), |sequence| sequence.name.clone()),
            source: SourceIdentity {
                uri: source_uri.to_owned(),
                source_hash: source_hash.clone(),
                importer: "metrocalk-gltf-animation".into(),
                importer_version: env!("CARGO_PKG_VERSION").into(),
                modified_epoch_ms: None,
            },
            content: ContentIdentity {
                content_hash: source_hash.clone(),
                canonical_hash: content_hash(compiled_hashes.join("\n").as_bytes()),
                byte_size: bytes.len() as u64,
            },
            facets: animation_facets,
        };
        facts.sort_by(|left, right| {
            left.location
                .cmp(&right.location)
                .then(left.code.cmp(&right.code))
        });
        let record = imported_asset_record(&manifest, &facts);
        Ok(AnimationAsset {
            sequences,
            manifest,
            record,
            targets,
            facts,
        })
    }
}

fn imported_asset_record(
    manifest: &AnimationAssetManifest,
    facts: &[AnimationImportFact],
) -> AnimationAssetRecord {
    let manifest_diagnostics = manifest.quality_diagnostics();
    let error_count = facts
        .iter()
        .filter(|fact| fact.severity == Severity::Error)
        .count()
        + manifest_diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .count();
    let warning_count = facts
        .iter()
        .filter(|fact| fact.severity == Severity::Warning)
        .count()
        + manifest_diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Warning)
            .count();
    let state = if error_count > 0 {
        AnimationImportState::Failed
    } else if warning_count > 0 {
        AnimationImportState::ReadyWithWarnings
    } else {
        AnimationImportState::Ready
    };
    let source_hash = manifest.source.source_hash.clone();
    let mut record = AnimationAssetRecord::from(manifest.clone());
    record.import_status = AnimationImportStatus {
        state,
        attempt: 1,
        last_successful_source_hash: (state != AnimationImportState::Failed)
            .then(|| source_hash.clone()),
        pending_source_hash: (state == AnimationImportState::Failed).then_some(source_hash),
        summary: match state {
            AnimationImportState::Ready => "imported and validated".into(),
            AnimationImportState::ReadyWithWarnings => {
                format!("imported with {warning_count} warning(s)")
            }
            AnimationImportState::Failed => {
                format!("import failed validation with {error_count} error(s)")
            }
            _ => unreachable!("fresh glTF ingestion only produces terminal import states"),
        },
    };
    record.editor = AnimationEditorRepresentation {
        primary_context: AnimationEditorContext::ThreeD,
        supported_contexts: BTreeSet::from([
            AnimationEditorContext::Generic,
            AnimationEditorContext::ThreeD,
            AnimationEditorContext::Cad,
        ]),
        icon: "animation-clip".into(),
        preview_clip: manifest
            .facets
            .iter()
            .flat_map(|facet| facet.clips.iter())
            .next()
            .map(|clip| clip.clip_id.clone()),
    };
    // glTF node, skin and morph references are embedded source identities here, not independent project
    // assets. Do not invent repository dependency IDs; the source reference produced by `From` records the
    // observable source URI/hash until a repository resolves real cross-asset edges.
    record.dependencies.clear();
    record.reimport_diagnostics = facts
        .iter()
        .map(|fact| AnimationReimportDiagnostic {
            severity: fact.severity,
            code: animation_import_code_name(fact.code).into(),
            channel_id: Some(fact.location.clone()),
            message: fact.message.clone(),
            remediation: fact.remediation.clone(),
            previous_revision: None,
            candidate_source_hash: Some(manifest.source.source_hash.clone()),
        })
        .collect();
    record
}

const fn animation_import_code_name(code: AnimationImportCode) -> &'static str {
    match code {
        AnimationImportCode::NoAnimations => "no_animations",
        AnimationImportCode::EmptyAnimation => "empty_animation",
        AnimationImportCode::UnresolvedBuffer => "unresolved_buffer",
        AnimationImportCode::EmptyInput => "empty_input",
        AnimationImportCode::NonFiniteTime => "non_finite_time",
        AnimationImportCode::NegativeTime => "negative_time",
        AnimationImportCode::NonMonotonicTime => "non_monotonic_time",
        AnimationImportCode::QuantizedTimeCollision => "quantized_time_collision",
        AnimationImportCode::OutputCardinalityMismatch => "output_cardinality_mismatch",
        AnimationImportCode::NonFiniteOutput => "non_finite_output",
        AnimationImportCode::InvalidQuaternion => "invalid_quaternion",
        AnimationImportCode::UnknownMorphTargetCount => "unknown_morph_target_count",
        AnimationImportCode::ElementLimitExceeded => "element_limit_exceeded",
        AnimationImportCode::KernelValidationFailed => "kernel_validation_failed",
        AnimationImportCode::MultipleSkeletons => "multiple_skeletons",
        AnimationImportCode::CrossSkeletonClip => "cross_skeleton_clip",
        AnimationImportCode::SurplusInverseBindMatrices => "surplus_inverse_bind_matrices",
        AnimationImportCode::UnsupportedAnimationPointer => "unsupported_animation_pointer",
        AnimationImportCode::StaticDurationPadded => "static_duration_padded",
    }
}

struct DecodedChannel {
    track: Track,
    maximum_tick: Tick,
    morph_target_count: usize,
}

struct ImportedClipProfile {
    animation_index: usize,
    summary: ClipSummary,
    capabilities: BTreeSet<AnimationCapability>,
    maximum_morph_targets: usize,
    skin_index: Option<usize>,
    channel_bindings: BTreeMap<String, String>,
    compiled_hash: String,
}

struct ChannelFailure {
    code: AnimationImportCode,
    message: String,
    remediation: &'static str,
}

fn decode_channel(
    channel: &gltf::animation::Channel<'_>,
    blob: &[u8],
    joint_nodes: &BTreeSet<usize>,
    target_ids: &HashMap<usize, String>,
    animation_index: usize,
    facts: &mut Vec<AnimationImportFact>,
) -> Result<DecodedChannel, ChannelFailure> {
    let channel_index = channel.index();
    let location = format!("animations[{animation_index}].channels[{channel_index}]");
    let input_count = channel.sampler().input().count();
    let output_count = channel.sampler().output().count();
    if input_count > MAX_ELEMENTS || output_count > MAX_ELEMENTS {
        return Err(ChannelFailure {
            code: AnimationImportCode::ElementLimitExceeded,
            message: format!(
                "channel declares {input_count} inputs and {output_count} outputs; limit is {MAX_ELEMENTS}"
            ),
            remediation: "Reduce or split the source animation before import.",
        });
    }
    let reader = channel.reader(|buffer| match buffer.source() {
        gltf::buffer::Source::Bin => Some(blob),
        gltf::buffer::Source::Uri(_) => None,
    });
    let Some(inputs) = reader.read_inputs() else {
        return Err(ChannelFailure {
            code: AnimationImportCode::UnresolvedBuffer,
            message: "input accessor buffer is unavailable in this self-contained ingestion tier"
                .into(),
            remediation: "Pack buffers into a GLB or resolve external buffers before ingestion.",
        });
    };
    let input_seconds: Vec<f32> = inputs.collect();
    if input_seconds.is_empty() {
        return Err(ChannelFailure {
            code: AnimationImportCode::EmptyInput,
            message: "channel has no input times".into(),
            remediation: "Author at least one keyframe time.",
        });
    }
    let mut ticks = Vec::with_capacity(input_seconds.len());
    let mut previous_seconds = None;
    for (key_index, seconds) in input_seconds.iter().copied().enumerate() {
        if !seconds.is_finite() {
            return Err(ChannelFailure {
                code: AnimationImportCode::NonFiniteTime,
                message: format!("key {key_index} time is NaN or infinity"),
                remediation: "Replace non-finite key times in the source DCC.",
            });
        }
        if seconds < 0.0 {
            return Err(ChannelFailure {
                code: AnimationImportCode::NegativeTime,
                message: format!("key {key_index} time {seconds} is negative"),
                remediation:
                    "Shift the glTF animation so its first key is at or after zero seconds.",
            });
        }
        if previous_seconds.is_some_and(|previous| seconds <= previous) {
            return Err(ChannelFailure {
                code: AnimationImportCode::NonMonotonicTime,
                message: format!("key {key_index} time is not strictly increasing"),
                remediation: "Sort keys and remove duplicate source times.",
            });
        }
        let tick = GLTF_ANIMATION_TIME_BASE
            .from_seconds(f64::from(seconds))
            .map_err(|error| ChannelFailure {
                code: AnimationImportCode::NonFiniteTime,
                message: error.to_string(),
                remediation: "Use finite animation times within the supported integer range.",
            })?;
        if ticks.last() == Some(&tick) {
            facts.push(fact(
                Severity::Warning,
                AnimationImportCode::QuantizedTimeCollision,
                &format!("{location}.keys[{key_index}]"),
                "distinct source times quantize to the same nanosecond tick; stable KeyId conflict resolution will apply",
                "Space the keys at least one nanosecond apart if both must remain independently sampleable.",
            ));
        }
        ticks.push(tick);
        previous_seconds = Some(seconds);
    }
    let Some(outputs) = reader.read_outputs() else {
        return Err(ChannelFailure {
            code: AnimationImportCode::UnresolvedBuffer,
            message: "output accessor buffer is unavailable in this self-contained ingestion tier"
                .into(),
            remediation: "Pack buffers into a GLB or resolve external buffers before ingestion.",
        });
    };

    let property = channel.target().property();
    let node = channel.target().node();
    let (kind, property_name, component, morph_target_count, raw_values) = match outputs {
        ReadOutputs::Translations(values) => (
            ValueKind::Vec3,
            "translation",
            if joint_nodes.contains(&node.index()) {
                "SkeletonJoint"
            } else {
                "Transform"
            },
            0,
            values
                .map(|value| AnimValue::Vec3(value.map(f64::from)))
                .collect::<Vec<_>>(),
        ),
        ReadOutputs::Rotations(values) => (
            ValueKind::Quaternion,
            "rotation",
            if joint_nodes.contains(&node.index()) {
                "SkeletonJoint"
            } else {
                "Transform"
            },
            0,
            values
                .into_f32()
                .map(|value| AnimValue::Quaternion(value.map(f64::from)))
                .collect::<Vec<_>>(),
        ),
        ReadOutputs::Scales(values) => (
            ValueKind::Vec3,
            "scale",
            if joint_nodes.contains(&node.index()) {
                "SkeletonJoint"
            } else {
                "Transform"
            },
            0,
            values
                .map(|value| AnimValue::Vec3(value.map(f64::from)))
                .collect::<Vec<_>>(),
        ),
        ReadOutputs::MorphTargetWeights(values) => {
            let width = morph_target_count_for_node(&node).ok_or(ChannelFailure {
                code: AnimationImportCode::UnknownMorphTargetCount,
                message: "weights channel target has no discoverable morph-target count".into(),
                remediation:
                    "Bind the node to a mesh with consistent morph targets and default weights.",
            })?;
            if width > MAX_ELEMENTS {
                return Err(ChannelFailure {
                    code: AnimationImportCode::ElementLimitExceeded,
                    message: format!("morph target count {width} exceeds limit {MAX_ELEMENTS}"),
                    remediation: "Reduce or split the morph target set before import.",
                });
            }
            let flattened: Vec<f64> = values.into_f32().map(f64::from).collect();
            let grouped = flattened
                .chunks_exact(width)
                .map(|chunk| AnimValue::Weights(chunk.to_vec()))
                .collect::<Vec<_>>();
            if !flattened.len().is_multiple_of(width) {
                return Err(ChannelFailure {
                    code: AnimationImportCode::OutputCardinalityMismatch,
                    message: format!(
                        "{} scalar weight outputs cannot be grouped into {width} morph targets",
                        flattened.len()
                    ),
                    remediation:
                        "Export one weight per morph target for every sampler output tuple.",
                });
            }
            (
                ValueKind::Weights,
                "weights",
                "MorphWeights",
                width,
                grouped,
            )
        }
    };

    let interpolation = match channel.sampler().interpolation() {
        GltfInterpolation::Step => Interpolation::Step,
        GltfInterpolation::Linear => Interpolation::Linear,
        GltfInterpolation::CubicSpline => Interpolation::Cubic,
    };
    let tuple_multiplier = if interpolation == Interpolation::Cubic {
        3
    } else {
        1
    };
    let expected_values = ticks.len().saturating_mul(tuple_multiplier);
    if raw_values.len() != expected_values {
        return Err(ChannelFailure {
            code: AnimationImportCode::OutputCardinalityMismatch,
            message: format!(
                "{:?} channel has {} decoded value tuples; expected {expected_values} for {} input times",
                channel.sampler().interpolation(),
                raw_values.len(),
                ticks.len()
            ),
            remediation: "Re-export the channel with one output per key, or three tangent/value tuples per CUBICSPLINE key.",
        });
    }
    if raw_values.iter().any(|value| !value.is_finite()) {
        return Err(ChannelFailure {
            code: AnimationImportCode::NonFiniteOutput,
            message: "channel output contains NaN or infinity".into(),
            remediation: "Replace non-finite animation values in the source DCC.",
        });
    }

    let mut keyframes = Vec::with_capacity(ticks.len());
    for (key_index, tick) in ticks.iter().copied().enumerate() {
        let mut key = if interpolation == Interpolation::Cubic {
            let base = key_index * 3;
            let mut key = Keyframe::new(
                KeyId::new(format!(
                    "gltf-key:{animation_index}:{channel_index}:{key_index}"
                )),
                tick,
                raw_values[base + 1].clone(),
            );
            key.in_tangent = Some(per_second_to_per_tick(&raw_values[base]));
            key.out_tangent = Some(per_second_to_per_tick(&raw_values[base + 2]));
            key
        } else {
            Keyframe::new(
                KeyId::new(format!(
                    "gltf-key:{animation_index}:{channel_index}:{key_index}"
                )),
                tick,
                raw_values[key_index].clone(),
            )
        };
        if let AnimValue::Quaternion(rotation) = &key.value {
            let length_squared = rotation.iter().map(|part| part * part).sum::<f64>();
            if !length_squared.is_finite() || length_squared <= 1.0e-24 {
                return Err(ChannelFailure {
                    code: AnimationImportCode::InvalidQuaternion,
                    message: format!("rotation key {key_index} has zero or invalid length"),
                    remediation: "Normalize source rotation keys and remove zero quaternions.",
                });
            }
        }
        // Preserve the explicit tangent fields only for cubic channels.
        if interpolation != Interpolation::Cubic {
            key.in_tangent = None;
            key.out_tangent = None;
        }
        keyframes.push(key);
    }
    debug_assert!(matches!(
        (property, kind),
        (Property::Translation | Property::Scale, ValueKind::Vec3)
            | (Property::Rotation, ValueKind::Quaternion)
            | (Property::MorphTargetWeights, ValueKind::Weights)
    ));
    let target_id = target_ids
        .get(&node.index())
        .cloned()
        .ok_or(ChannelFailure {
            code: AnimationImportCode::KernelValidationFailed,
            message: format!("node {} has no canonical target identity", node.index()),
            remediation: "Re-import the complete validated glTF hierarchy.",
        })?;
    let track_id = TrackId::new(format!("gltf-track:{animation_index}:{channel_index}"));
    Ok(DecodedChannel {
        maximum_tick: *ticks.last().expect("non-empty input was checked"),
        morph_target_count,
        track: Track {
            id: track_id,
            name: format!("{} {property_name}", node.name().unwrap_or(&target_id)),
            binding: Binding {
                path: PropertyPath::new(target_id, component, property_name),
                value_kind: kind,
            },
            interpolation,
            keyframes,
            enabled: true,
        },
    })
}

fn per_second_to_per_tick(value: &AnimValue) -> AnimValue {
    let divisor = f64::from(GLTF_ANIMATION_TIME_BASE.ticks_per_second);
    match value {
        AnimValue::Number(value) => AnimValue::Number(value / divisor),
        AnimValue::Integer(value) => AnimValue::Integer(*value),
        AnimValue::Boolean(value) => AnimValue::Boolean(*value),
        AnimValue::String(value) => AnimValue::String(value.clone()),
        AnimValue::Vec3(value) => AnimValue::Vec3(value.map(|part| part / divisor)),
        AnimValue::Vec4(value) => AnimValue::Vec4(value.map(|part| part / divisor)),
        AnimValue::Quaternion(value) => AnimValue::Quaternion(value.map(|part| part / divisor)),
        AnimValue::Weights(value) => {
            AnimValue::Weights(value.iter().map(|part| part / divisor).collect())
        }
    }
}

fn morph_target_count_for_node(node: &gltf::Node<'_>) -> Option<usize> {
    node.weights()
        .map(<[f32]>::len)
        .or_else(|| {
            node.mesh()
                .and_then(|mesh| mesh.weights().map(<[f32]>::len))
        })
        .or_else(|| {
            node.mesh()
                .and_then(|mesh| mesh.primitives().next())
                .map(|primitive| primitive.morph_targets().count())
                .filter(|count| *count > 0)
        })
}

fn target_identities(
    doc: &Gltf,
    canonical_paths: &HashMap<usize, String>,
) -> Result<Vec<AnimationTargetIdentity>, ImportError> {
    let mut memberships: HashMap<usize, Vec<(u32, u32)>> = HashMap::new();
    for skin in doc.skins() {
        for (slot, node) in skin.joints().enumerate() {
            memberships.entry(node.index()).or_default().push((
                u32::try_from(skin.index()).expect("skin count was guarded by MAX_ELEMENTS"),
                u32::try_from(slot).expect("joint count is bounded by guarded node count"),
            ));
        }
    }
    doc.nodes()
        .map(|node| {
            let canonical_path = canonical_paths
                .get(&node.index())
                .ok_or_else(|| {
                    ImportError::Malformed(format!(
                        "nodes[{}] has no canonical hierarchy path",
                        node.index()
                    ))
                })?
                .clone();
            Ok(AnimationTargetIdentity {
                id: format!(
                    "gltf-target:{}",
                    content_hash(canonical_path.as_bytes())
                        .strip_prefix("fnv128:")
                        .expect("content hash prefix is stable")
                ),
                source_node_index: u32::try_from(node.index())
                    .expect("node count was guarded by MAX_ELEMENTS"),
                display_name: node
                    .name()
                    .map_or_else(|| "Unnamed node".into(), str::to_owned),
                canonical_path,
                joint_memberships: memberships.remove(&node.index()).unwrap_or_default(),
            })
        })
        .collect()
}

#[derive(Debug)]
struct ValidatedNodeHierarchy {
    parent_by_node: Vec<Option<usize>>,
    children_by_node: Vec<Vec<usize>>,
    roots: Vec<usize>,
}

impl ValidatedNodeHierarchy {
    fn from_document(doc: &Gltf) -> Result<Self, ImportError> {
        let node_count = doc.nodes().count();
        guard_document_count(node_count)?;
        let mut parent_by_node = vec![None; node_count];
        let mut children_by_node = vec![Vec::new(); node_count];

        for node in doc.nodes() {
            let parent = node.index();
            if parent >= node_count {
                return Err(ImportError::Malformed(format!(
                    "nodes[{parent}] is outside the declared node table"
                )));
            }
            for child in node.children() {
                let child = child.index();
                if child >= node_count {
                    return Err(ImportError::Malformed(format!(
                        "nodes[{parent}].children references missing nodes[{child}]"
                    )));
                }
                if let Some(existing_parent) = parent_by_node[child] {
                    return Err(ImportError::Malformed(format!(
                        "nodes[{child}] has more than one hierarchy parent: nodes[{existing_parent}] and nodes[{parent}]"
                    )));
                }
                parent_by_node[child] = Some(parent);
                children_by_node[parent].push(child);
            }
        }

        // A node has at most one parent now, so walking parent links gives a bounded O(nodes)
        // cycle check. States left by a completed walk are marked done before the next start.
        let mut states = vec![0_u8; node_count];
        for start in 0..node_count {
            if states[start] == 2 {
                continue;
            }
            let mut chain = Vec::new();
            let mut cursor = Some(start);
            while let Some(index) = cursor {
                match states[index] {
                    0 => {
                        states[index] = 1;
                        chain.push(index);
                        if chain.len() > node_count {
                            return Err(ImportError::Malformed(
                                "node hierarchy traversal exceeded the node count".into(),
                            ));
                        }
                        cursor = parent_by_node[index];
                    }
                    1 => {
                        return Err(ImportError::Malformed(format!(
                            "node hierarchy contains a cycle through nodes[{index}]"
                        )));
                    }
                    2 => break,
                    _ => unreachable!("hierarchy state is internal"),
                }
            }
            for index in chain {
                states[index] = 2;
            }
        }

        let roots = parent_by_node
            .iter()
            .enumerate()
            .filter_map(|(index, parent)| parent.is_none().then_some(index))
            .collect();
        Ok(Self {
            parent_by_node,
            children_by_node,
            roots,
        })
    }
}

fn validate_skin_joint_tables(doc: &Gltf) -> Result<(), ImportError> {
    let mut total_joint_references = 0_usize;
    for skin in doc.skins() {
        let mut joints = BTreeSet::new();
        for joint in skin.joints() {
            total_joint_references = total_joint_references.checked_add(1).ok_or_else(|| {
                ImportError::Malformed("skin joint reference count overflow".into())
            })?;
            guard_document_count(total_joint_references)?;
            if !joints.insert(joint.index()) {
                return Err(ImportError::Malformed(format!(
                    "skins[{}].joints lists nodes[{}] more than once",
                    skin.index(),
                    joint.index()
                )));
            }
        }
    }
    Ok(())
}

const MAX_CANONICAL_HIERARCHY_DEPTH: usize = 4_096;
const MAX_CANONICAL_PATH_BYTES: usize = 64 * 1_024;
const MAX_TOTAL_CANONICAL_PATH_BYTES: usize = MAX_IMPORT_BYTES;

fn canonical_node_paths(
    doc: &Gltf,
    hierarchy: &ValidatedNodeHierarchy,
) -> Result<HashMap<usize, String>, ImportError> {
    let nodes: Vec<_> = doc.nodes().collect();
    let names: HashMap<_, _> = nodes
        .iter()
        .map(|node| {
            (
                node.index(),
                node.name().unwrap_or("Unnamed node").to_owned(),
            )
        })
        .collect();
    let mut ordinal_by_node = vec![None; nodes.len()];
    for siblings in std::iter::once(hierarchy.roots.as_slice())
        .chain(hierarchy.children_by_node.iter().map(Vec::as_slice))
    {
        let mut next_ordinal_by_name: HashMap<&str, usize> = HashMap::new();
        for node in siblings {
            let name = names[node].as_str();
            let next = next_ordinal_by_name.entry(name).or_default();
            ordinal_by_node[*node] = Some(*next);
            *next += 1;
        }
    }

    // Build each path exactly once in top-down order. Depth, individual path bytes, and aggregate
    // retained bytes are admission-bounded so a valid-but-hostile deep hierarchy cannot create
    // quadratic traversal work or unbounded canonical identity storage.
    let mut canonical_paths: HashMap<usize, String> = HashMap::with_capacity(nodes.len());
    let mut depth_by_node = vec![0_usize; nodes.len()];
    let mut total_path_bytes = 0_usize;
    let mut pending: Vec<_> = hierarchy.roots.iter().rev().copied().collect();
    while let Some(node) = pending.pop() {
        let ordinal = ordinal_by_node[node].ok_or_else(|| {
            ImportError::Malformed(format!(
                "nodes[{node}] is missing from its validated sibling list"
            ))
        })?;
        let escaped_name = escape_path_segment(&names[&node]);
        let segment = format!("{escaped_name}#{ordinal}");
        let (depth, path_bytes) = if let Some(parent) = hierarchy.parent_by_node[node] {
            let parent_path = canonical_paths.get(&parent).ok_or_else(|| {
                ImportError::Malformed(format!(
                    "nodes[{node}] was reached before its hierarchy parent nodes[{parent}]"
                ))
            })?;
            (
                depth_by_node[parent] + 1,
                parent_path
                    .len()
                    .checked_add(1)
                    .and_then(|length| length.checked_add(segment.len()))
                    .ok_or_else(|| {
                        ImportError::Malformed(format!(
                            "canonical hierarchy path length overflow at nodes[{node}]"
                        ))
                    })?,
            )
        } else {
            (1, segment.len())
        };
        if depth > MAX_CANONICAL_HIERARCHY_DEPTH {
            return Err(ImportError::Malformed(format!(
                "node hierarchy depth {depth} at nodes[{node}] exceeds the admission limit {MAX_CANONICAL_HIERARCHY_DEPTH}"
            )));
        }
        if path_bytes > MAX_CANONICAL_PATH_BYTES {
            return Err(ImportError::Malformed(format!(
                "canonical path for nodes[{node}] is {path_bytes} bytes (limit {MAX_CANONICAL_PATH_BYTES})"
            )));
        }
        total_path_bytes = total_path_bytes.checked_add(path_bytes).ok_or_else(|| {
            ImportError::Malformed("canonical hierarchy byte count overflow".into())
        })?;
        if total_path_bytes > MAX_TOTAL_CANONICAL_PATH_BYTES {
            return Err(ImportError::Malformed(format!(
                "canonical hierarchy paths retain {total_path_bytes} bytes (limit {MAX_TOTAL_CANONICAL_PATH_BYTES})"
            )));
        }

        let path = if let Some(parent) = hierarchy.parent_by_node[node] {
            let parent_path = &canonical_paths[&parent];
            let mut path = String::with_capacity(path_bytes);
            path.push_str(parent_path);
            path.push('/');
            path.push_str(&segment);
            path
        } else {
            segment
        };
        depth_by_node[node] = depth;
        canonical_paths.insert(node, path);
        pending.extend(hierarchy.children_by_node[node].iter().rev().copied());
    }
    if canonical_paths.len() != nodes.len() {
        return Err(ImportError::Malformed(format!(
            "validated node hierarchy reached {} of {} nodes",
            canonical_paths.len(),
            nodes.len()
        )));
    }
    Ok(canonical_paths)
}

fn escape_path_segment(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('/', "%2F")
        .replace('#', "%23")
}

const MAX_SKELETON_SIGNATURE_WORK_UNITS: usize = 1_000_000;
const MAX_SKELETON_SIGNATURE_BYTES: usize = MAX_IMPORT_BYTES;

#[derive(Debug)]
struct SkeletonAdmissionBudget {
    consumed_work_units: usize,
    consumed_signature_bytes: usize,
    maximum_work_units: usize,
    maximum_signature_bytes: usize,
}

impl Default for SkeletonAdmissionBudget {
    fn default() -> Self {
        Self {
            consumed_work_units: 0,
            consumed_signature_bytes: 0,
            maximum_work_units: MAX_SKELETON_SIGNATURE_WORK_UNITS,
            maximum_signature_bytes: MAX_SKELETON_SIGNATURE_BYTES,
        }
    }
}

impl SkeletonAdmissionBudget {
    #[cfg(test)]
    fn with_limits(maximum_work_units: usize, maximum_signature_bytes: usize) -> Self {
        Self {
            maximum_work_units,
            maximum_signature_bytes,
            ..Self::default()
        }
    }

    fn charge_work(&mut self, units: usize, skin_index: usize) -> Result<(), ImportError> {
        self.consumed_work_units = self
            .consumed_work_units
            .checked_add(units)
            .ok_or_else(|| ImportError::Malformed("skeleton work counter overflow".into()))?;
        if self.consumed_work_units > self.maximum_work_units {
            return Err(ImportError::Malformed(format!(
                "skins[{skin_index}] exceeds the cumulative skeleton-signature work limit {}",
                self.maximum_work_units
            )));
        }
        Ok(())
    }

    fn charge_signature_bytes(
        &mut self,
        bytes: usize,
        skin_index: usize,
    ) -> Result<(), ImportError> {
        self.consumed_signature_bytes = self
            .consumed_signature_bytes
            .checked_add(bytes)
            .ok_or_else(|| {
                ImportError::Malformed("skeleton signature byte counter overflow".into())
            })?;
        if self.consumed_signature_bytes > self.maximum_signature_bytes {
            return Err(ImportError::Malformed(format!(
                "skins[{skin_index}] exceeds the cumulative skeleton-signature byte limit {}",
                self.maximum_signature_bytes
            )));
        }
        Ok(())
    }
}

fn build_skeleton_signatures(
    doc: &Gltf,
    blob: &[u8],
    hierarchy: &ValidatedNodeHierarchy,
    canonical_paths: &HashMap<usize, String>,
    local_matrices: &[[[f32; 4]; 4]],
    budget: &mut SkeletonAdmissionBudget,
) -> Result<HashMap<usize, SkeletonSignature>, ImportError> {
    let mut signatures = HashMap::new();
    for skin in doc.skins() {
        signatures.insert(
            skin.index(),
            skeleton_signature(
                &skin,
                blob,
                hierarchy,
                canonical_paths,
                local_matrices,
                budget,
            )?,
        );
    }
    Ok(signatures)
}

fn skeleton_signature(
    skin: &gltf::Skin<'_>,
    blob: &[u8],
    hierarchy: &ValidatedNodeHierarchy,
    canonical_paths: &HashMap<usize, String>,
    local_matrices: &[[[f32; 4]; 4]],
    budget: &mut SkeletonAdmissionBudget,
) -> Result<SkeletonSignature, ImportError> {
    let source_joints: Vec<_> = skin.joints().collect();
    let mut joint_nodes = BTreeSet::new();
    for joint in &source_joints {
        if !joint_nodes.insert(joint.index()) {
            return Err(ImportError::Malformed(format!(
                "skins[{}].joints lists nodes[{}] more than once",
                skin.index(),
                joint.index()
            )));
        }
    }

    let inverse_binds = read_inverse_bind_matrices(skin, blob, source_joints.len())?;
    let mut joints = Vec::with_capacity(source_joints.len());
    for (node, inverse_bind) in source_joints.into_iter().zip(inverse_binds) {
        budget.charge_work(1, skin.index())?;
        let mut intermediary_nodes = Vec::new();
        let mut parent_joint = None;
        let mut cursor = hierarchy.parent_by_node[node.index()];
        for _ in 0..hierarchy.parent_by_node.len() {
            let Some(candidate) = cursor else {
                break;
            };
            budget.charge_work(1, skin.index())?;
            if joint_nodes.contains(&candidate) {
                parent_joint = Some(candidate);
                break;
            }
            intermediary_nodes.push(candidate);
            cursor = hierarchy.parent_by_node[candidate];
        }
        if parent_joint.is_none() && cursor.is_some() {
            return Err(ImportError::Malformed(format!(
                "skins[{}] ancestor traversal from nodes[{}] exceeded the node count",
                skin.index(),
                node.index()
            )));
        }
        let canonical_path = canonical_paths.get(&node.index()).ok_or_else(|| {
            ImportError::Malformed(format!(
                "skins[{}].joints references nodes[{}] without a canonical path",
                skin.index(),
                node.index()
            ))
        })?;
        joints.push((
            canonical_path.as_str(),
            node.name().unwrap_or("Unnamed joint").to_owned(),
            node.index(),
            parent_joint,
            inverse_bind,
            intermediary_nodes,
        ));
    }
    joints.sort_by(|left, right| left.0.cmp(right.0));
    let canonical_index_by_node: HashMap<_, _> = joints
        .iter()
        .enumerate()
        .map(|(index, joint)| (joint.2, index))
        .collect();
    let joint_names: Vec<_> = joints.iter().map(|joint| joint.1.clone()).collect();
    let parent_indices: Vec<_> = joints
        .iter()
        .map(|joint| {
            joint.3.map(|parent| {
                u32::try_from(canonical_index_by_node[&parent])
                    .expect("joint count is bounded by guarded node count")
            })
        })
        .collect();
    let mut signature_bytes = Vec::new();
    for joint in &joints {
        frame_bytes(
            &mut signature_bytes,
            joint.0.as_bytes(),
            budget,
            skin.index(),
        )?;
        match joint.3 {
            Some(parent) => frame_bytes(
                &mut signature_bytes,
                canonical_paths[&parent].as_bytes(),
                budget,
                skin.index(),
            )?,
            None => frame_bytes(&mut signature_bytes, &[], budget, skin.index())?,
        }
    }
    let mut rest_bytes = Vec::new();
    for joint in &joints {
        frame_bytes(&mut rest_bytes, joint.0.as_bytes(), budget, skin.index())?;
        // Hash the complete effective-local chain between parent joints. A non-joint intermediary is
        // part of the bind pose even when the source omits inverseBindMatrices.
        let chain_length = joint.5.len() + 1;
        budget.charge_signature_bytes(size_of::<u64>(), skin.index())?;
        rest_bytes.extend_from_slice(&(chain_length as u64).to_le_bytes());
        for node_index in joint
            .5
            .iter()
            .rev()
            .copied()
            .chain(std::iter::once(joint.2))
        {
            frame_bytes(
                &mut rest_bytes,
                canonical_paths[&node_index].as_bytes(),
                budget,
                skin.index(),
            )?;
            let local_matrix = local_matrices.get(node_index).ok_or_else(|| {
                ImportError::Malformed(format!(
                    "nodes[{node_index}] has no precomputed local transform"
                ))
            })?;
            budget.charge_signature_bytes(16 * size_of::<u32>(), skin.index())?;
            for column in local_matrix {
                for value in column {
                    rest_bytes.extend_from_slice(&canonical_f32_bits(*value).to_le_bytes());
                }
            }
        }
        budget.charge_signature_bytes(16 * size_of::<u32>(), skin.index())?;
        for column in joint.4 {
            for value in column {
                rest_bytes.extend_from_slice(&canonical_f32_bits(value).to_le_bytes());
            }
        }
    }
    Ok(SkeletonSignature {
        signature_hash: content_hash(&signature_bytes),
        rest_pose_hash: content_hash(&rest_bytes),
        joint_names,
        parent_indices,
        humanoid_profile: None,
    })
}

fn read_inverse_bind_matrices(
    skin: &gltf::Skin<'_>,
    blob: &[u8],
    joint_count: usize,
) -> Result<Vec<[[f32; 4]; 4]>, ImportError> {
    let Some(accessor) = skin.inverse_bind_matrices() else {
        return Ok(vec![
            [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0]
            ];
            joint_count
        ]);
    };
    if accessor.count() < joint_count {
        return Err(ImportError::Malformed(format!(
            "skins[{}].inverseBindMatrices accessor {} declares {} matrices for {} joints; the accessor must cover every joint",
            skin.index(),
            accessor.index(),
            accessor.count(),
            joint_count
        )));
    }
    let matrices = skin
        .reader(|buffer| match buffer.source() {
            gltf::buffer::Source::Bin => Some(blob),
            gltf::buffer::Source::Uri(_) => None,
        })
        .read_inverse_bind_matrices()
        .ok_or_else(|| {
            ImportError::Malformed(format!(
                "skins[{}].inverseBindMatrices accessor {} could not be read from an embedded binary buffer",
                skin.index(),
                accessor.index()
            ))
        })?
        .take(joint_count)
        .collect::<Vec<_>>();
    if matrices.len() < joint_count {
        return Err(ImportError::Malformed(format!(
            "skins[{}].inverseBindMatrices accessor {} decoded {} matrices for {} joints; the accessor must cover every joint",
            skin.index(),
            accessor.index(),
            matrices.len(),
            joint_count
        )));
    }
    Ok(matrices)
}

fn inverse_bind_matrix_surplus_facts(doc: &Gltf) -> Vec<AnimationImportFact> {
    doc.skins()
        .filter_map(|skin| {
            let accessor = skin.inverse_bind_matrices()?;
            let joint_count = skin.joints().count();
            (accessor.count() > joint_count).then(|| {
                fact(
                    Severity::Warning,
                    AnimationImportCode::SurplusInverseBindMatrices,
                    &format!("skins[{}].inverseBindMatrices", skin.index()),
                    format!(
                        "inverse-bind accessor {} declares {} matrices for {} joints; only the first {joint_count} matrices are addressable by skin.joints and were retained",
                        accessor.index(),
                        accessor.count(),
                        joint_count
                    ),
                    "Remove unused trailing inverse-bind matrices to keep the source minimal; the retained joint palette is unchanged.",
                )
            })
        })
        .collect()
}

fn frame_bytes(
    output: &mut Vec<u8>,
    value: &[u8],
    budget: &mut SkeletonAdmissionBudget,
    skin_index: usize,
) -> Result<(), ImportError> {
    budget.charge_signature_bytes(size_of::<u64>() + value.len(), skin_index)?;
    output.extend_from_slice(&(value.len() as u64).to_le_bytes());
    output.extend_from_slice(value);
    Ok(())
}

fn canonical_f32_bits(value: f32) -> u32 {
    if value == 0.0 {
        0
    } else if value.is_nan() {
        f32::NAN.to_bits()
    } else {
        value.to_bits()
    }
}

fn fact(
    severity: Severity,
    code: AnimationImportCode,
    location: &str,
    message: impl Into<String>,
    remediation: impl Into<String>,
) -> AnimationImportFact {
    AnimationImportFact {
        severity,
        code,
        location: location.into(),
        message: message.into(),
        remediation: remediation.into(),
    }
}

fn guard_document_count(count: usize) -> Result<(), ImportError> {
    if count > MAX_ELEMENTS {
        Err(ImportError::TooManyElements {
            count,
            limit: MAX_ELEMENTS,
        })
    } else {
        Ok(())
    }
}

fn contains_ascii(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn extension_required_ascii(bytes: &[u8], extension: &[u8]) -> bool {
    let marker = b"extensionsRequired";
    let Some(start) = bytes
        .windows(marker.len())
        .position(|window| window == marker)
    else {
        return false;
    };
    let tail = &bytes[start + marker.len()..];
    let Some(end) = tail.iter().position(|byte| *byte == b']') else {
        return false;
    };
    contains_ascii(&tail[..=end], extension)
}

fn content_hash(bytes: &[u8]) -> String {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u128::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("fnv128:{hash:032x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixtureBuilder {
        bin: Vec<u8>,
        views: Vec<String>,
        accessors: Vec<String>,
        skins: String,
    }

    impl FixtureBuilder {
        fn new() -> Self {
            Self {
                bin: Vec::new(),
                views: Vec::new(),
                accessors: Vec::new(),
                skins: "[{\"joints\":[0]}]".into(),
            }
        }

        fn with_skins(mut self, skins: impl Into<String>) -> Self {
            self.skins = skins.into();
            self
        }

        fn f32_accessor(
            &mut self,
            values: &[f32],
            count: usize,
            element_type: &str,
            bounds: Option<(&str, &str)>,
        ) -> usize {
            while !self.bin.len().is_multiple_of(4) {
                self.bin.push(0);
            }
            let offset = self.bin.len();
            for value in values {
                self.bin.extend_from_slice(&value.to_le_bytes());
            }
            let view = self.views.len();
            self.views.push(format!(
                "{{\"buffer\":0,\"byteOffset\":{offset},\"byteLength\":{}}}",
                values.len() * 4
            ));
            let accessor = self.accessors.len();
            let bounds = bounds.map_or_else(String::new, |(min, max)| {
                format!(",\"min\":{min},\"max\":{max}")
            });
            self.accessors.push(format!(
                "{{\"bufferView\":{view},\"componentType\":5126,\"count\":{count},\"type\":\"{element_type}\"{bounds}}}"
            ));
            accessor
        }

        fn finish(self, invalid_translation_count: bool, pointer_extension: bool) -> Vec<u8> {
            self.finish_with_cubic_curve(invalid_translation_count, pointer_extension, false)
        }

        fn finish_with_cubic_curve(
            self,
            invalid_translation_count: bool,
            pointer_extension: bool,
            authored_hemisphere_curve: bool,
        ) -> Vec<u8> {
            let mut builder = self;
            let positions = builder.f32_accessor(
                &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                3,
                "VEC3",
                Some(("[0,0,0]", "[1,1,0]")),
            );
            let morph_a = builder.f32_accessor(&[0.0; 9], 3, "VEC3", None);
            let morph_b = builder.f32_accessor(&[0.0; 9], 3, "VEC3", None);
            let times = builder.f32_accessor(&[0.0, 1.0], 2, "SCALAR", None);
            let translation_values: &[f32] = if invalid_translation_count {
                &[0.0, 0.0, 0.0]
            } else {
                &[0.0, 0.0, 0.0, 2.0, 4.0, 6.0]
            };
            let translation = builder.f32_accessor(
                translation_values,
                if invalid_translation_count { 1 } else { 2 },
                "VEC3",
                None,
            );
            // CUBICSPLINE: [in tangent, value, out tangent] for each key. The alternate curve uses
            // antipodal endpoint values deliberately: its component polynomial must not be hemisphere-flipped.
            let ordinary_rotation = [
                0.0, 0.0, 0.0, 0.0, // key 0 in
                0.0, 0.0, 0.0, 1.0, // key 0 value
                0.0, 0.0, 1.0, 0.0, // key 0 out / second
                0.0, 0.0, 1.0, 0.0, // key 1 in / second
                0.0, 0.0, 1.0, 0.0, // key 1 value
                0.0, 0.0, 0.0, 0.0, // key 1 out
            ];
            let authored_rotation = [
                0.0, 0.0, 0.0, 0.0, // key 0 in
                0.0, 0.0, 0.0, 1.0, // key 0 value (+W)
                1.0, 0.0, 0.0, 0.0, // key 0 out / second
                0.0, 0.0, 0.0, 0.0, // key 1 in
                0.0, 0.0, 0.0, -1.0, // key 1 value (-W), authored hemisphere
                0.0, 0.0, 0.0, 0.0, // key 1 out
            ];
            let rotation_values = if authored_hemisphere_curve {
                &authored_rotation
            } else {
                &ordinary_rotation
            };
            let rotation = builder.f32_accessor(rotation_values, 6, "VEC4", None);
            let scale = builder.f32_accessor(&[1.0, 1.0, 1.0, 2.0, 2.0, 2.0], 2, "VEC3", None);
            let weights = builder.f32_accessor(&[0.0, 1.0, 1.0, 0.0], 4, "SCALAR", None);
            let extension = if pointer_extension {
                ",\"extensionsUsed\":[\"KHR_animation_pointer\"]"
            } else {
                ""
            };
            let json = format!(
                "{{\"asset\":{{\"version\":\"2.0\"}}{extension},\
                 \"buffers\":[{{\"byteLength\":{}}}],\
                 \"bufferViews\":[{}],\"accessors\":[{}],\
                 \"meshes\":[{{\"weights\":[0,0],\"primitives\":[{{\"attributes\":{{\"POSITION\":{positions}}},\"targets\":[{{\"POSITION\":{morph_a}}},{{\"POSITION\":{morph_b}}}]}}]}}],\
                 \"nodes\":[{{\"name\":\"Hip\",\"mesh\":0,\"skin\":0}}],\"skins\":{skins},\
                 \"animations\":[{{\"name\":\"Hero Motion\",\"samplers\":[\
                   {{\"input\":{times},\"output\":{translation},\"interpolation\":\"LINEAR\"}},\
                   {{\"input\":{times},\"output\":{rotation},\"interpolation\":\"CUBICSPLINE\"}},\
                   {{\"input\":{times},\"output\":{scale},\"interpolation\":\"STEP\"}},\
                   {{\"input\":{times},\"output\":{weights},\"interpolation\":\"LINEAR\"}}],\
                 \"channels\":[\
                   {{\"sampler\":0,\"target\":{{\"node\":0,\"path\":\"translation\"}}}},\
                   {{\"sampler\":1,\"target\":{{\"node\":0,\"path\":\"rotation\"}}}},\
                   {{\"sampler\":2,\"target\":{{\"node\":0,\"path\":\"scale\"}}}},\
                   {{\"sampler\":3,\"target\":{{\"node\":0,\"path\":\"weights\"}}}}]}}]}}",
                builder.bin.len(),
                builder.views.join(","),
                builder.accessors.join(","),
                skins = builder.skins,
            );
            glb(json.as_bytes(), &builder.bin)
        }
    }

    fn glb(json: &[u8], bin: &[u8]) -> Vec<u8> {
        let mut json_chunk = json.to_vec();
        while !json_chunk.len().is_multiple_of(4) {
            json_chunk.push(b' ');
        }
        let mut bin_chunk = bin.to_vec();
        while !bin_chunk.len().is_multiple_of(4) {
            bin_chunk.push(0);
        }
        let total = 12 + 8 + json_chunk.len() + 8 + bin_chunk.len();
        let mut bytes = Vec::with_capacity(total);
        bytes.extend_from_slice(b"glTF");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(
            &u32::try_from(total)
                .expect("test GLB fits u32")
                .to_le_bytes(),
        );
        bytes.extend_from_slice(
            &u32::try_from(json_chunk.len())
                .expect("test JSON fits u32")
                .to_le_bytes(),
        );
        bytes.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
        bytes.extend_from_slice(&json_chunk);
        bytes.extend_from_slice(
            &u32::try_from(bin_chunk.len())
                .expect("test BIN fits u32")
                .to_le_bytes(),
        );
        bytes.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        bytes.extend_from_slice(&bin_chunk);
        bytes
    }

    #[test]
    fn imports_core_channels_and_preserves_authored_semantics() {
        let bytes = FixtureBuilder::new().finish(false, false);
        let asset = GltfImporter::new()
            .import_animations_with_source(&bytes, "project://hero.glb")
            .expect("animation import");
        assert!(!asset.has_errors(), "facts: {:?}", asset.facts);
        assert_eq!(asset.sequences.len(), 1);
        let sequence = &asset.sequences[0];
        assert_eq!(sequence.name, "Hero Motion");
        assert_eq!(sequence.duration, Tick(1_000_000_000));
        assert_eq!(sequence.tracks.len(), 4);
        assert_eq!(
            sequence
                .tracks
                .iter()
                .map(|track| track.interpolation)
                .collect::<Vec<_>>(),
            vec![
                Interpolation::Linear,
                Interpolation::Cubic,
                Interpolation::Step,
                Interpolation::Linear,
            ]
        );
        assert_eq!(sequence.tracks[0].binding.path.target, asset.targets[0].id);
        assert!(sequence.tracks[0]
            .binding
            .path
            .target
            .starts_with("gltf-target:"));
        assert_eq!(asset.targets[0].canonical_path, "Hip#0");
        assert_eq!(sequence.tracks[0].binding.path.component, "SkeletonJoint");
        let rotation = &sequence.tracks[1];
        let AnimValue::Quaternion(out_tangent) = rotation.keyframes[0]
            .out_tangent
            .as_ref()
            .expect("cubic outgoing tangent")
        else {
            panic!("quaternion tangent")
        };
        assert!((out_tangent[2] - 1.0e-9).abs() < 1.0e-15);
        let AnimValue::Weights(weights) = &sequence.tracks[3].keyframes[1].value else {
            panic!("morph weights")
        };
        assert_eq!(weights, &[1.0, 0.0]);

        let facet = &asset.manifest.facets[0];
        assert!(facet
            .capabilities
            .contains(&AnimationCapability::CubicInterpolation));
        assert!(facet
            .capabilities
            .contains(&AnimationCapability::MorphTargets));
        assert_eq!(facet.morphs.as_ref().unwrap().target_count, 2);
        assert_eq!(facet.clips[0].duration, Tick(1_000_000_000));
        assert_eq!(asset.manifest.source.uri, "project://hero.glb");
        assert_eq!(asset.record.manifest, asset.manifest);
        assert_eq!(
            asset.record.effective_logical_id(),
            &asset.manifest.asset_id
        );
        assert!(matches!(
            asset.record.import_status.state,
            AnimationImportState::Ready | AnimationImportState::ReadyWithWarnings
        ));
        assert_eq!(asset.targets[0].joint_memberships, vec![(0, 0)]);
        assert!(facet.skeleton_signature.is_some());
        assert!(
            !facet.reimport.preserves_authored_overrides,
            "canonical target identity is stable, but channel-order override migration remains revision-fenced"
        );
        assert!(facet.reimport.strategy.contains("canonical-hierarchy"));
        assert!(facet.reimport.strategy.contains("revision-fenced"));
        sequence.compile().expect("kernel-valid imported sequence");
    }

    fn mapped_request(
        asset: &AnimationAsset,
        instance_id: &str,
        target_prefix: &str,
    ) -> ClipInstantiationRequest {
        let clip_id = asset.manifest.facets[0].clips[0].clip_id.clone();
        let source = asset.clip_binding_signature(&clip_id).unwrap();
        let target_by_source: BTreeMap<_, _> = source
            .bindings
            .iter()
            .map(|binding| {
                (
                    binding.path.target.clone(),
                    format!("{target_prefix}:{}", binding.path.target),
                )
            })
            .collect();
        let target_bindings: Vec<_> = source
            .bindings
            .iter()
            .map(|binding| {
                let mut target = binding.clone();
                target.path.target = target_by_source[&binding.path.target].clone();
                target
            })
            .collect();
        let target_element_counts = source
            .element_counts
            .iter()
            .map(|(binding, count)| {
                let mut target = binding.clone();
                target.path.target = target_by_source[&binding.path.target].clone();
                (target, *count)
            })
            .collect();
        let mut request = ClipInstantiationRequest::new(
            instance_id,
            clip_id,
            BindingSignature::with_element_counts(target_bindings, target_element_counts),
        );
        request.expected_source_binding_hash = Some(source.signature_hash);
        request.target_rebinds = target_by_source
            .into_iter()
            .map(|(source_target, target_target)| {
                metrocalk_animation::TargetRebind::new(source_target, target_target)
            })
            .collect();
        request.target_skeleton = asset.manifest.facets[0].skeleton_signature.clone();
        request
    }

    #[test]
    fn imported_clip_instances_are_revision_fenced_and_target_isolated() {
        let asset = GltfImporter::new()
            .import_animations(&FixtureBuilder::new().finish(false, false))
            .unwrap();
        let left_request = mapped_request(&asset, "clip-instance:hero-a", "entity:hero-a");
        let right_request = mapped_request(&asset, "clip-instance:hero-b", "entity:hero-b");
        let left = asset
            .instantiate_clip(&left_request)
            .expect("left instance");
        let right = asset
            .instantiate_clip(&right_request)
            .expect("right instance");

        assert_eq!(left.plan.id.as_str(), "clip-instance:hero-a");
        assert_eq!(right.plan.id.as_str(), "clip-instance:hero-b");
        assert_ne!(left.stable_hash, right.stable_hash);
        assert_eq!(
            left.source.sequence_id, right.source.sequence_id,
            "instances share immutable source provenance"
        );
        assert!(left
            .plan
            .evaluate(Tick::ZERO)
            .bindings
            .iter()
            .all(|binding| binding.binding.path.target.starts_with("entity:hero-a:")));
        assert!(right
            .plan
            .evaluate(Tick::ZERO)
            .bindings
            .iter()
            .all(|binding| binding.binding.path.target.starts_with("entity:hero-b:")));
        assert!(asset.sequences[0].tracks.iter().all(|track| track
            .binding
            .path
            .target
            .starts_with("gltf-target:")));
    }

    #[test]
    fn stale_source_signature_and_unknown_clip_fail_with_stable_codes() {
        let asset = GltfImporter::new()
            .import_animations(&FixtureBuilder::new().finish(false, false))
            .unwrap();
        let mut request = mapped_request(&asset, "clip-instance:stale", "entity:stale");
        request.expected_source_binding_hash = Some("mtkbind:stale".into());
        let error = asset.instantiate_clip(&request).unwrap_err();
        assert!(error
            .issues
            .iter()
            .any(|issue| issue.code == ClipInstantiationCode::SourceBindingSignatureMismatch));

        request.clip_id = ClipId::new("clip:missing");
        let error = asset.instantiate_clip(&request).unwrap_err();
        assert_eq!(error.issues[0].code, ClipInstantiationCode::MissingClip);
    }

    #[test]
    fn content_qualified_sequence_and_clip_ids_do_not_collide_across_assets() {
        let first_bytes = FixtureBuilder::new().finish(false, false);
        let mut second_bytes = first_bytes.clone();
        let marker = b"Hero Motion";
        let offset = second_bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("fixture animation name");
        second_bytes[offset + marker.len() - 1] = b'N';
        let importer = GltfImporter::new();
        let first = importer.import_animations(&first_bytes).unwrap();
        let second = importer.import_animations(&second_bytes).unwrap();
        assert_ne!(first.sequences[0].id, second.sequences[0].id);
        assert_ne!(
            first.manifest.facets[0].clips[0].clip_id,
            second.manifest.facets[0].clips[0].clip_id
        );
    }

    #[test]
    fn same_source_reimport_preserves_logical_identity_but_fences_revision_and_clips() {
        let first_bytes = FixtureBuilder::new().finish(false, false);
        let mut changed_bytes = first_bytes.clone();
        let marker = b"Hero Motion";
        let offset = changed_bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("fixture animation name");
        changed_bytes[offset + marker.len() - 1] = b'N';
        let importer = GltfImporter::new();
        let logical_id = AnimationAssetId::new("animation:hero-motion");
        let first = importer
            .import_animations_with_logical_id(
                &first_bytes,
                "project://characters/hero.glb",
                logical_id.clone(),
            )
            .unwrap();
        let changed = importer
            .import_animations_with_logical_id(
                &changed_bytes,
                "project://characters/hero.glb",
                logical_id,
            )
            .unwrap();
        assert_eq!(
            first.record.effective_logical_id(),
            changed.record.effective_logical_id()
        );
        assert_ne!(
            first.record.effective_revision_id(),
            changed.record.effective_revision_id()
        );
        assert_ne!(first.sequences[0].id, changed.sequences[0].id);
        assert_ne!(
            first.manifest.facets[0].clips[0].clip_id,
            changed.manifest.facets[0].clips[0].clip_id
        );

        assert_eq!(
            changed.record.effective_logical_id().as_str(),
            "animation:hero-motion"
        );
    }

    #[test]
    fn malformed_node_cycle_is_rejected_before_canonical_path_construction() {
        let error = GltfImporter::new()
            .import_animations(
                br#"{
                  "asset":{"version":"2.0"},
                  "nodes":[
                    {"name":"A","children":[1]},
                    {"name":"B","children":[0]}
                  ]
                }"#,
            )
            .unwrap_err();
        assert!(
            matches!(error, ImportError::Malformed(ref message) if message.contains("hierarchy contains a cycle")),
            "got {error:?}"
        );
    }

    #[test]
    fn multiply_parented_node_is_rejected_before_target_identity_construction() {
        let error = GltfImporter::new()
            .import_animations(
                br#"{
                  "asset":{"version":"2.0"},
                  "nodes":[
                    {"name":"Parent A","children":[2]},
                    {"name":"Parent B","children":[2]},
                    {"name":"Child"}
                  ]
                }"#,
            )
            .unwrap_err();
        assert!(
            matches!(error, ImportError::Malformed(ref message) if message.contains("more than one hierarchy parent")),
            "got {error:?}"
        );
    }

    #[test]
    fn duplicate_skin_joint_is_rejected_before_membership_expansion() {
        let error = GltfImporter::new()
            .import_animations(
                br#"{
                  "asset":{"version":"2.0"},
                  "nodes":[{"name":"Hip"}],
                  "skins":[{"joints":[0,0]}]
                }"#,
            )
            .unwrap_err();
        assert!(
            matches!(error, ImportError::Malformed(ref message) if message.contains("lists nodes[0] more than once")),
            "got {error:?}"
        );
    }

    #[test]
    fn absent_inverse_bind_accessor_uses_identity_matrices() {
        let doc = Gltf::from_slice(
            br#"{
              "asset":{"version":"2.0"},
              "nodes":[{"name":"Hip"}],
              "skins":[{"joints":[0]}]
            }"#,
        )
        .unwrap();
        let hierarchy = ValidatedNodeHierarchy::from_document(&doc).unwrap();
        let canonical_paths = canonical_node_paths(&doc, &hierarchy).unwrap();
        let local_matrices: Vec<_> = doc.nodes().map(|node| node.transform().matrix()).collect();
        let mut budget = SkeletonAdmissionBudget::default();
        let signature = skeleton_signature(
            &doc.skins().next().unwrap(),
            &[],
            &hierarchy,
            &canonical_paths,
            &local_matrices,
            &mut budget,
        )
        .expect("an omitted accessor has the glTF-defined identity semantics");
        assert!(!signature.rest_pose_hash.is_empty());
    }

    #[test]
    fn declared_but_unresolved_inverse_bind_accessor_is_not_treated_as_identity() {
        let error = GltfImporter::new()
            .import_animations(
                br#"{
                  "asset":{"version":"2.0"},
                  "buffers":[{"byteLength":64,"uri":"missing-inverse-binds.bin"}],
                  "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":64}],
                  "accessors":[{"bufferView":0,"componentType":5126,"count":1,"type":"MAT4"}],
                  "nodes":[{"name":"Hip"}],
                  "skins":[{"inverseBindMatrices":0,"joints":[0]}]
                }"#,
            )
            .unwrap_err();
        assert!(
            matches!(error, ImportError::Malformed(ref message) if message.contains("could not be read from an embedded binary buffer")),
            "got {error:?}"
        );
    }

    #[test]
    fn inverse_bind_matrix_count_must_cover_joint_count() {
        let matrix = [
            1.0_f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let bin = matrix
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        let bytes = glb(
            br#"{
              "asset":{"version":"2.0"},
              "buffers":[{"byteLength":64}],
              "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":64}],
              "accessors":[{"bufferView":0,"componentType":5126,"count":1,"type":"MAT4"}],
              "nodes":[
                {"name":"Hip","children":[1]},
                {"name":"Knee"}
              ],
              "skins":[{"inverseBindMatrices":0,"joints":[0,1]}]
            }"#,
            &bin,
        );
        let error = GltfImporter::new().import_animations(&bytes).unwrap_err();
        assert!(
            matches!(error, ImportError::Malformed(ref message) if message.contains("declares 1 matrices for 2 joints")),
            "got {error:?}"
        );
    }

    #[test]
    fn surplus_inverse_bind_matrices_are_accepted_and_reported() {
        let matrix = [
            1.0_f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let bin = matrix
            .into_iter()
            .chain(matrix)
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        let bytes = glb(
            br#"{
              "asset":{"version":"2.0"},
              "buffers":[{"byteLength":128}],
              "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":128}],
              "accessors":[{"bufferView":0,"componentType":5126,"count":2,"type":"MAT4"}],
              "nodes":[{"name":"Hip"}],
              "skins":[{"inverseBindMatrices":0,"joints":[0]}]
            }"#,
            &bin,
        );
        let asset = GltfImporter::new()
            .import_animations(&bytes)
            .expect("surplus inverse binds are valid glTF");
        let surplus = asset
            .facts
            .iter()
            .find(|fact| fact.code == AnimationImportCode::SurplusInverseBindMatrices)
            .expect("surplus inverse binds remain visible");
        assert_eq!(surplus.severity, Severity::Warning);
        assert!(surplus.message.contains("only the first 1 matrices"));
        assert!(!surplus.remediation.is_empty());
    }

    #[test]
    fn cumulative_skeleton_work_budget_bounds_many_deep_skins() {
        let doc = Gltf::from_slice(
            br#"{
              "asset":{"version":"2.0"},
              "nodes":[
                {"name":"Root","children":[1]},
                {"name":"Offset A","children":[2]},
                {"name":"Offset B","children":[3]},
                {"name":"Leaf"}
              ],
              "skins":[
                {"joints":[3]},
                {"joints":[3]},
                {"joints":[3]}
              ]
            }"#,
        )
        .unwrap();
        let hierarchy = ValidatedNodeHierarchy::from_document(&doc).unwrap();
        let canonical_paths = canonical_node_paths(&doc, &hierarchy).unwrap();
        let local_matrices: Vec<_> = doc.nodes().map(|node| node.transform().matrix()).collect();
        let mut budget = SkeletonAdmissionBudget::with_limits(8, usize::MAX);
        let error = build_skeleton_signatures(
            &doc,
            &[],
            &hierarchy,
            &canonical_paths,
            &local_matrices,
            &mut budget,
        )
        .unwrap_err();
        assert!(
            matches!(error, ImportError::Malformed(ref message)
                if message.contains("skins[2]")
                    && message.contains("cumulative skeleton-signature work limit")),
            "got {error:?}"
        );
    }

    #[test]
    fn canonical_skeleton_signature_ignores_skin_joint_declaration_order() {
        let first = br#"{
          "asset":{"version":"2.0"},
          "nodes":[
            {"name":"Hip","children":[1]},
            {"name":"Knee","translation":[0,-1,0]}
          ],
          "skins":[{"joints":[0,1]}]
        }"#;
        let reordered = br#"{
          "asset":{"version":"2.0"},
          "nodes":[
            {"name":"Hip","children":[1]},
            {"name":"Knee","translation":[0,-1,0]}
          ],
          "skins":[{"joints":[1,0]}]
        }"#;
        let changed_rest = br#"{
          "asset":{"version":"2.0"},
          "nodes":[
            {"name":"Hip","children":[1]},
            {"name":"Knee","translation":[0,-2,0]}
          ],
          "skins":[{"joints":[0,1]}]
        }"#;
        let signature = |bytes: &[u8]| {
            let doc = Gltf::from_slice(bytes).unwrap();
            let skin = doc.skins().next().unwrap();
            let hierarchy = ValidatedNodeHierarchy::from_document(&doc).unwrap();
            let canonical_paths = canonical_node_paths(&doc, &hierarchy).unwrap();
            let local_matrices: Vec<_> =
                doc.nodes().map(|node| node.transform().matrix()).collect();
            skeleton_signature(
                &skin,
                &[],
                &hierarchy,
                &canonical_paths,
                &local_matrices,
                &mut SkeletonAdmissionBudget::default(),
            )
            .unwrap()
        };
        let first = signature(first);
        let reordered = signature(reordered);
        let changed_rest = signature(changed_rest);
        assert_eq!(first, reordered);
        assert_eq!(first.signature_hash, changed_rest.signature_hash);
        assert_ne!(first.rest_pose_hash, changed_rest.rest_pose_hash);
    }

    #[test]
    fn skeleton_rest_signature_includes_non_joint_intermediary_transforms() {
        let signature = |offset: i32| {
            let json = format!(
                r#"{{
                  "asset":{{"version":"2.0"}},
                  "nodes":[
                    {{"name":"Hip","children":[1]}},
                    {{"name":"RigOffset","children":[2],"translation":[0,{offset},0]}},
                    {{"name":"Knee"}}
                  ],
                  "skins":[{{"joints":[0,2]}}]
                }}"#
            );
            let doc = Gltf::from_slice(json.as_bytes()).unwrap();
            let skin = doc.skins().next().unwrap();
            let hierarchy = ValidatedNodeHierarchy::from_document(&doc).unwrap();
            let canonical_paths = canonical_node_paths(&doc, &hierarchy).unwrap();
            let local_matrices: Vec<_> =
                doc.nodes().map(|node| node.transform().matrix()).collect();
            skeleton_signature(
                &skin,
                &[],
                &hierarchy,
                &canonical_paths,
                &local_matrices,
                &mut SkeletonAdmissionBudget::default(),
            )
            .unwrap()
        };
        let first = signature(1);
        let changed = signature(2);
        assert_eq!(first.signature_hash, changed.signature_hash);
        assert_ne!(
            first.rest_pose_hash, changed.rest_pose_hash,
            "an intermediary node changes the effective child-joint bind pose even without authored inverse binds"
        );
    }

    #[test]
    fn canonical_target_identity_survives_unrelated_node_reindexing() {
        let first = Gltf::from_slice(
            br#"{
              "asset":{"version":"2.0"},
              "nodes":[
                {"name":"Hip","children":[1]},
                {"name":"Knee"},
                {"name":"Prop"}
              ]
            }"#,
        )
        .unwrap();
        let reordered = Gltf::from_slice(
            br#"{
              "asset":{"version":"2.0"},
              "nodes":[
                {"name":"Prop"},
                {"name":"Hip","children":[2]},
                {"name":"Knee"}
              ]
            }"#,
        )
        .unwrap();
        let identities = |doc: &Gltf| {
            let hierarchy = ValidatedNodeHierarchy::from_document(doc).unwrap();
            let canonical_paths = canonical_node_paths(doc, &hierarchy).unwrap();
            target_identities(doc, &canonical_paths)
                .unwrap()
                .into_iter()
                .map(|target| (target.canonical_path, target.id))
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(identities(&first), identities(&reordered));
    }

    #[test]
    fn cross_skin_clip_is_rejected_instead_of_using_the_first_skin() {
        let asset = GltfImporter::new()
            .import_animations(
                &FixtureBuilder::new()
                    .with_skins("[{\"joints\":[0]},{\"joints\":[0]}]")
                    .finish(false, false),
            )
            .unwrap();
        assert!(asset.facts.iter().any(|fact| {
            fact.code == AnimationImportCode::CrossSkeletonClip && fact.severity == Severity::Error
        }));
        assert!(!asset.is_ready_for_playback());
        assert!(asset.manifest.facets[0].skeleton_signature.is_none());
    }

    #[test]
    fn malformed_channel_is_skipped_with_actionable_fact_not_silent_loss() {
        let bytes = FixtureBuilder::new().finish(true, false);
        let asset = GltfImporter::new()
            .import_animations(&bytes)
            .expect("document itself remains importable");
        assert_eq!(asset.sequences[0].tracks.len(), 3);
        let failure = asset
            .facts
            .iter()
            .find(|fact| fact.code == AnimationImportCode::OutputCardinalityMismatch)
            .expect("cardinality fact");
        assert_eq!(failure.severity, Severity::Error);
        assert_eq!(
            asset.record.import_status.state,
            AnimationImportState::Failed
        );
        assert!(failure.location.contains("channels[0]"));
        assert!(!failure.remediation.is_empty());
    }

    #[test]
    fn playback_readiness_includes_import_facts_and_manifest_quality() {
        let importer = GltfImporter::new();
        let valid = importer
            .import_animations(&FixtureBuilder::new().finish(false, false))
            .expect("valid animation");
        let facet = &valid.manifest.facets[0];
        let target = PlaybackTarget {
            capabilities: facet.capabilities.clone(),
            skeleton: facet.skeleton_signature.clone(),
            maximum_morph_targets: 64,
            extensions: facet
                .extensions
                .iter()
                .map(|extension| extension.namespace.clone())
                .collect(),
        };
        assert!(valid.is_ready_for_playback());
        assert!(valid.can_play_on(&target));

        let fatal_channel = importer
            .import_animations(&FixtureBuilder::new().finish(true, false))
            .expect("document remains inspectable");
        assert!(fatal_channel
            .facts
            .iter()
            .any(|fact| fact.severity == Severity::Error));
        assert!(fatal_channel.has_errors());
        assert!(!fatal_channel.is_ready_for_playback());
        assert!(
            !fatal_channel.can_play_on(&target),
            "target capability compatibility cannot hide a fatal import fact"
        );

        let no_animation = importer
            .import_animations(br#"{"asset":{"version":"2.0"}}"#)
            .expect("a static glTF remains inspectable");
        assert!(no_animation
            .facts
            .iter()
            .all(|fact| fact.severity != Severity::Error));
        assert!(
            no_animation.has_errors(),
            "NoClips is a fatal manifest-quality error even though ingestion emitted only a warning"
        );
        assert!(!no_animation.is_ready_for_playback());
    }

    #[test]
    fn gltf_cubic_rotation_keeps_authored_endpoint_signs_and_tangents() {
        let bytes = FixtureBuilder::new().finish_with_cubic_curve(false, false, true);
        let asset = GltfImporter::new().import_animations(&bytes).unwrap();
        let compiled = asset.sequences[0].compile().unwrap();
        let evaluation = compiled.evaluate(Tick(500_000_000));
        let rotation = evaluation
            .bindings
            .iter()
            .find(|sample| sample.binding.path.property == "rotation")
            .expect("rotation sample");
        let AnimValue::Quaternion(value) = &rotation.value else {
            panic!("quaternion sample")
        };
        // glTF component Hermite produces [0.125, 0, 0, 0], normalized to +X. Flipping the authored
        // -W endpoint would produce a mostly-W quaternion and fail this conformance assertion.
        assert!((value[0] - 1.0).abs() < 1.0e-12, "{value:?}");
        assert!(value[1].abs() < 1.0e-12);
        assert!(value[2].abs() < 1.0e-12);
        assert!(
            value[3].abs() < 1.0e-12,
            "authored endpoint was flipped: {value:?}"
        );
    }

    #[test]
    fn unsupported_animation_pointer_is_retained_as_manifest_extension_and_fact() {
        let bytes = FixtureBuilder::new().finish(false, true);
        let asset = GltfImporter::new().import_animations(&bytes).unwrap();
        assert!(asset.facts.iter().any(|fact| {
            fact.code == AnimationImportCode::UnsupportedAnimationPointer
                && fact.severity == Severity::Warning
        }));
        assert_eq!(
            asset.manifest.facets[0].extensions[0].namespace,
            "KHR_animation_pointer"
        );
        assert!(!asset.manifest.facets[0].extensions[0].required);
        assert_eq!(
            asset.record.import_status.state,
            AnimationImportState::ReadyWithWarnings
        );
        assert!(asset.record.reimport_diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "unsupported_animation_pointer"
                && diagnostic.severity == Severity::Warning
        }));
    }

    #[test]
    fn unresolved_external_buffers_are_explicit_channel_facts() {
        let json = br#"{
          "asset":{"version":"2.0"},
          "buffers":[{"byteLength":32,"uri":"motion.bin"}],
          "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":8},{"buffer":0,"byteOffset":8,"byteLength":24}],
          "accessors":[
            {"bufferView":0,"componentType":5126,"count":2,"type":"SCALAR"},
            {"bufferView":1,"componentType":5126,"count":2,"type":"VEC3"}
          ],
          "nodes":[{"name":"External"}],
          "animations":[{"samplers":[{"input":0,"output":1}],"channels":[{"sampler":0,"target":{"node":0,"path":"translation"}}]}]
        }"#;
        let asset = GltfImporter::new().import_animations(json).unwrap();
        assert!(asset.facts.iter().any(|fact| {
            fact.code == AnimationImportCode::UnresolvedBuffer
                && fact.message.contains("self-contained")
        }));
        assert!(asset.sequences[0].tracks.is_empty());
    }

    #[test]
    fn repeated_ingestion_has_identical_ids_manifests_and_sequences() {
        let bytes = FixtureBuilder::new().finish(false, false);
        let importer = GltfImporter::new();
        let first = importer.import_animations(&bytes).unwrap();
        let second = importer.import_animations(&bytes).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.sequences[0].compile().unwrap().stable_hash,
            second.sequences[0].compile().unwrap().stable_hash
        );
    }

    #[test]
    fn legacy_json_without_lifecycle_record_migrates_without_losing_manifest_identity() {
        let asset = GltfImporter::new()
            .import_animations_with_source(
                &FixtureBuilder::new().finish(false, true),
                "project://legacy.glb",
            )
            .expect("animation import");
        let mut legacy = serde_json::to_value(&asset).expect("serialize current asset");
        legacy
            .as_object_mut()
            .expect("asset object")
            .remove("record");

        let migrated: AnimationAsset =
            serde_json::from_value(legacy).expect("migrate earlier JSON shape");
        assert_eq!(migrated.manifest, asset.manifest);
        assert_eq!(migrated.record.manifest, migrated.manifest);
        assert_eq!(
            migrated.record.effective_logical_id(),
            &migrated.manifest.asset_id
        );
        assert_eq!(
            migrated.record.import_status.state,
            AnimationImportState::ReadyWithWarnings
        );
        assert!(migrated.record.source.watch_for_changes);
    }
}
