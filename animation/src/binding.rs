//! Shared animation-binding vocabulary.
//!
//! A [`PropertyPath`] remains the only address in the animation system. The types in this module
//! describe how an editor should present that path and whether a runtime can consume it; they do
//! not introduce a second target or property model.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{PropertyPath, Severity, ValueKind};

/// The authoring/runtime domain in which a binding is meaningful.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationDomain {
    /// Sprite, tile, and other two-dimensional scene animation.
    TwoD,
    /// Spatial, skeletal, camera, light, and CAD motion animation.
    ThreeD,
    /// Screen-space and world-space user-interface animation.
    Ui,
    /// Domain-neutral values such as simulation or product-motion parameters.
    Generic,
}

/// The editor control best suited to authoring a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationEditorKind {
    Scalar,
    Integer,
    Toggle,
    Text,
    Vector3,
    Quaternion,
    Color,
    Opacity,
    Visibility,
    SpriteFrame,
    /// The binding is shown for explanation but is not editable.
    ReadOnly,
}

/// Stable meaning of a property, independent of its component's storage spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationSemantic {
    Unknown,
    Generic,
    Translation,
    Rotation,
    Scale,
    JointValue,
    SpriteTexture,
    SpriteLayer,
    SpriteFlip,
    SpriteFrame,
    UiLayout,
    UiAnchor,
    Opacity,
    Color,
    Visibility,
    LightKind,
    LightIntensity,
    LightRange,
    LightDirection,
    LightShadows,
    CameraFieldOfView,
    CameraClipPlane,
    CameraActive,
}

/// A verified animation projection that can consume evaluated values today.
///
/// Future component families deliberately do not appear here until their projection is wired. A
/// catalog entry with `runtime_sink: None` is therefore an honest unsupported entry, not a promise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationRuntimeSink {
    TransformProjection,
    KinematicJointProjection,
    /// A real editor preview consumer for sprite channels; packaged game rendering remains a separate gate.
    EditorTwoDProjection,
    /// A real editor preview consumer for UI layout/style channels; packaged game rendering remains a gate.
    EditorUiProjection,
}

/// Whether the editor may advertise a catalog entry as animatable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationReadiness {
    /// The schema and runtime projection are both usable without target-specific checks.
    Ready,
    /// Authorable and consumed by an editor preview adapter, but not claimed as packaged runtime coverage.
    PreviewOnly,
    /// A runtime projection exists, but the selected target must first pass a semantic check.
    RequiresValidation,
    /// The property is known but must remain non-authorable until the reported gap is fixed.
    Unsupported,
}

impl AnimationReadiness {
    /// Only unconditional entries should appear as immediately animatable in a generic catalog.
    #[must_use]
    pub const fn is_advertised_animatable(self) -> bool {
        matches!(self, Self::Ready | Self::PreviewOnly)
    }

    /// Whether target-aware validation could produce a usable runtime binding.
    #[must_use]
    pub const fn has_runtime_path(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::PreviewOnly | Self::RequiresValidation
        )
    }
}

/// Stable diagnostic code for binding discovery and schema/runtime reconciliation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationBindingDiagnosticCode {
    UnknownBinding,
    ComponentMetadataMissing,
    PropertyMetadataMissing,
    PropertyTypeMismatch,
    RuntimeSinkUnavailable,
    TargetValidationRequired,
    ProtectedReference,
}

/// A user-facing explanation attached to a binding descriptor or resolved context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationBindingDiagnostic {
    pub severity: Severity,
    pub code: AnimationBindingDiagnosticCode,
    pub message: String,
    pub remediation: String,
}

impl AnimationBindingDiagnostic {
    #[must_use]
    pub fn new(
        severity: Severity,
        code: AnimationBindingDiagnosticCode,
        message: impl Into<String>,
        remediation: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code,
            message: message.into(),
            remediation: remediation.into(),
        }
    }
}

/// Target-neutral catalog metadata for one component/property pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationBindingDescriptor {
    pub component: String,
    pub property: String,
    pub label: String,
    pub value_kind: ValueKind,
    pub domains: BTreeSet<AnimationDomain>,
    pub editor_kind: AnimationEditorKind,
    pub semantic: AnimationSemantic,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_sink: Option<AnimationRuntimeSink>,
    pub readiness: AnimationReadiness,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<AnimationBindingDiagnostic>,
}

impl AnimationBindingDescriptor {
    /// Resolve this descriptor over an existing property address. A mismatch is rejected so callers
    /// cannot accidentally decorate one path with another property's metadata.
    #[must_use]
    pub fn context_for(&self, path: PropertyPath) -> Option<AnimationBindingContext> {
        if path.component != self.component || path.property != self.property {
            return None;
        }
        Some(AnimationBindingContext {
            path,
            label: self.label.clone(),
            value_kind: Some(self.value_kind),
            domains: self.domains.clone(),
            editor_kind: self.editor_kind,
            semantic: self.semantic,
            runtime_sink: self.runtime_sink,
            readiness: self.readiness,
            diagnostics: self.diagnostics.clone(),
        })
    }

    /// True only when a generic editor can expose the entry and a sink is actually declared.
    #[must_use]
    pub const fn is_advertised_animatable(&self) -> bool {
        self.readiness.is_advertised_animatable() && self.runtime_sink.is_some()
    }
}

/// Editor/runtime metadata resolved over exactly one canonical [`PropertyPath`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimationBindingContext {
    pub path: PropertyPath,
    pub label: String,
    /// `None` is reserved for an unknown path returned by registry lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_kind: Option<ValueKind>,
    pub domains: BTreeSet<AnimationDomain>,
    pub editor_kind: AnimationEditorKind,
    pub semantic: AnimationSemantic,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_sink: Option<AnimationRuntimeSink>,
    pub readiness: AnimationReadiness,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<AnimationBindingDiagnostic>,
}

impl AnimationBindingContext {
    /// Build the fail-closed result for a path absent from a registry.
    #[must_use]
    pub fn unknown(path: PropertyPath) -> Self {
        let label = format!("{} / {}", path.component, path.property);
        Self {
            path,
            label,
            value_kind: None,
            domains: BTreeSet::new(),
            editor_kind: AnimationEditorKind::ReadOnly,
            semantic: AnimationSemantic::Unknown,
            runtime_sink: None,
            readiness: AnimationReadiness::Unsupported,
            diagnostics: vec![AnimationBindingDiagnostic::new(
                Severity::Error,
                AnimationBindingDiagnosticCode::UnknownBinding,
                "This property has no registered animation binding.",
                "Register a typed descriptor and a verified runtime sink before authoring keys.",
            )],
        }
    }

    #[must_use]
    pub const fn is_advertised_animatable(&self) -> bool {
        self.readiness.is_advertised_animatable() && self.runtime_sink.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_is_serializable_metadata_over_the_same_property_path() {
        let path =
            PropertyPath::new("entity:hero", "Transform", "translation").with_subpath(["local"]);
        let descriptor = AnimationBindingDescriptor {
            component: "Transform".into(),
            property: "translation".into(),
            label: "Position".into(),
            value_kind: ValueKind::Vec3,
            domains: [AnimationDomain::TwoD, AnimationDomain::ThreeD]
                .into_iter()
                .collect(),
            editor_kind: AnimationEditorKind::Vector3,
            semantic: AnimationSemantic::Translation,
            runtime_sink: Some(AnimationRuntimeSink::TransformProjection),
            readiness: AnimationReadiness::Ready,
            diagnostics: Vec::new(),
        };
        let context = descriptor.context_for(path.clone()).unwrap();
        assert_eq!(context.path, path);
        assert!(context.is_advertised_animatable());
        let encoded = serde_json::to_string(&context).unwrap();
        assert_eq!(
            serde_json::from_str::<AnimationBindingContext>(&encoded).unwrap(),
            context
        );
    }

    #[test]
    fn readiness_never_advertises_conditional_or_unsupported_entries() {
        assert!(AnimationReadiness::Ready.is_advertised_animatable());
        assert!(AnimationReadiness::PreviewOnly.is_advertised_animatable());
        assert!(!AnimationReadiness::RequiresValidation.is_advertised_animatable());
        assert!(!AnimationReadiness::Unsupported.is_advertised_animatable());
        assert!(AnimationReadiness::RequiresValidation.has_runtime_path());
        assert!(!AnimationReadiness::Unsupported.has_runtime_path());
    }

    #[test]
    fn descriptor_rejects_a_different_property_path() {
        let descriptor = AnimationBindingDescriptor {
            component: "Transform".into(),
            property: "x".into(),
            label: "Position X".into(),
            value_kind: ValueKind::Number,
            domains: [AnimationDomain::TwoD, AnimationDomain::ThreeD]
                .into_iter()
                .collect(),
            editor_kind: AnimationEditorKind::Scalar,
            semantic: AnimationSemantic::Translation,
            runtime_sink: Some(AnimationRuntimeSink::TransformProjection),
            readiness: AnimationReadiness::Ready,
            diagnostics: Vec::new(),
        };
        assert!(descriptor
            .context_for(PropertyPath::new("entity:hero", "Transform", "y"))
            .is_none());
    }
}
