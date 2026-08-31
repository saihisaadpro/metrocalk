//! Shared animation binding catalog for editor discovery.
//!
//! This module reconciles the pure animation descriptors with `/core` component metadata. It is
//! intentionally target-neutral: instance-specific checks (notably distinguishing a legacy
//! kinematic `Joint` from a physics joint) remain at the intent/runtime boundary.
#![expect(
    clippy::vec_init_then_push,
    reason = "this is a TABLE - one multi-line row per animatable channel. A `vec![]` literal of twenty               such rows reads and diffs worse than a list of pushes, which is the whole reason it is               written this way."
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use metrocalk_animation::{
    AnimationBindingContext, AnimationBindingDescriptor, AnimationBindingDiagnostic,
    AnimationBindingDiagnosticCode, AnimationDomain, AnimationEditorKind, AnimationReadiness,
    AnimationRuntimeSink, AnimationSemantic, PropertyPath, Severity, ValueKind,
};
use metrocalk_core::{stdlib::standard_components, ComponentMeta, FieldType};

/// Deterministic lookup table for built-in animation properties.
#[derive(Clone, Debug)]
pub struct AnimationBindingRegistry {
    descriptors: BTreeMap<(String, String), AnimationBindingDescriptor>,
}

impl Default for AnimationBindingRegistry {
    fn default() -> Self {
        Self::standard()
    }
}

impl AnimationBindingRegistry {
    /// Build the engine's standard catalog and reconcile it with `/core`'s standard metadata.
    #[must_use]
    pub fn standard() -> Self {
        let metadata = standard_components();
        Self::with_component_metadata(&metadata)
    }

    /// Build the standard descriptors against a supplied component catalog.
    ///
    /// This is the plugin/marketplace seam: mismatched declared types fail closed, while a verified
    /// runtime alias absent from the scalar core schema stays usable with a visible drift warning.
    #[must_use]
    pub fn with_component_metadata(component_metadata: &[ComponentMeta]) -> Self {
        let metadata: BTreeMap<_, _> = component_metadata
            .iter()
            .map(|component| (component.name.as_str(), component))
            .collect();
        let descriptors = built_in_animation_binding_descriptors()
            .into_iter()
            .map(|descriptor| reconcile_metadata(descriptor, &metadata))
            .map(|descriptor| {
                (
                    (descriptor.component.clone(), descriptor.property.clone()),
                    descriptor,
                )
            })
            .collect();
        Self { descriptors }
    }

    /// Look up a target-neutral descriptor.
    #[must_use]
    pub fn descriptor(
        &self,
        component: &str,
        property: &str,
    ) -> Option<&AnimationBindingDescriptor> {
        self.descriptors
            .get(&(component.to_owned(), property.to_owned()))
    }

    /// Resolve metadata over the supplied canonical path. Unknown paths return a fail-closed context
    /// carrying an actionable diagnostic rather than disappearing from the UI.
    #[must_use]
    pub fn resolve(&self, path: &PropertyPath) -> AnimationBindingContext {
        self.descriptor(&path.component, &path.property)
            .and_then(|descriptor| descriptor.context_for(path.clone()))
            .unwrap_or_else(|| AnimationBindingContext::unknown(path.clone()))
    }

    /// All descriptors in deterministic component/property order.
    pub fn catalog(&self) -> impl Iterator<Item = &AnimationBindingDescriptor> {
        self.descriptors.values()
    }

    /// Descriptors for one component in deterministic property order.
    pub fn catalog_for_component<'a>(
        &'a self,
        component: &'a str,
    ) -> impl Iterator<Item = &'a AnimationBindingDescriptor> + 'a {
        self.descriptors
            .range((component.to_owned(), String::new())..)
            .take_while(move |((candidate, _), _)| candidate == component)
            .map(|(_, descriptor)| descriptor)
    }

    /// Entries a generic property picker may honestly advertise without target-specific validation.
    pub fn advertised(&self) -> impl Iterator<Item = &AnimationBindingDescriptor> {
        self.catalog()
            .filter(|descriptor| descriptor.is_advertised_animatable())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }
}

/// Process-wide immutable standard registry. Runtime binding checks use this to avoid rebuilding the
/// descriptor table in the preview hot path; plugin-specific registries remain explicit values.
#[must_use]
pub fn standard_animation_binding_registry() -> &'static AnimationBindingRegistry {
    static REGISTRY: OnceLock<AnimationBindingRegistry> = OnceLock::new();
    REGISTRY.get_or_init(AnimationBindingRegistry::standard)
}

/// The built-in semantic catalog before `/core` metadata reconciliation.
#[must_use]
#[allow(clippy::too_many_lines)] // Explicit data table: every advertised property is audit-visible.
pub fn built_in_animation_binding_descriptors() -> Vec<AnimationBindingDescriptor> {
    use AnimationDomain::{Generic, ThreeD, TwoD, Ui};
    use AnimationEditorKind::{
        Color, Integer, Opacity, Quaternion, Scalar, Text, Toggle, Vector3, Visibility,
    };
    use AnimationSemantic::{
        CameraActive, CameraClipPlane, CameraFieldOfView, JointValue, LightDirection,
        LightIntensity, LightKind, LightRange, LightShadows, Rotation, Scale, SpriteFlip,
        SpriteLayer, SpriteTexture, Translation, UiAnchor, UiLayout,
    };

    let spatial = [TwoD, ThreeD];
    let mut out = Vec::new();

    out.push(ready(
        "Transform",
        "translation",
        "Position",
        ValueKind::Vec3,
        &spatial,
        Vector3,
        Translation,
        AnimationRuntimeSink::TransformProjection,
    ));
    out.push(ready(
        "Transform",
        "rotation",
        "Rotation",
        ValueKind::Quaternion,
        &spatial,
        Quaternion,
        Rotation,
        AnimationRuntimeSink::TransformProjection,
    ));
    out.push(ready(
        "Transform",
        "scale",
        "Uniform scale",
        ValueKind::Number,
        &spatial,
        Scalar,
        Scale,
        AnimationRuntimeSink::TransformProjection,
    ));
    // Imported glTF scale channels are typed Vec3 even though this editor stores a uniform scalar. The
    // clip-instancing gate maps only curves proven finite, positive and uniform onto this virtual sink;
    // it never becomes an authored ECS field.
    out.push(ready(
        "Transform",
        "scale3",
        "Imported uniform scale",
        ValueKind::Vec3,
        &spatial,
        Vector3,
        Scale,
        AnimationRuntimeSink::TransformProjection,
    ));
    for property in ["x", "y", "z", "px", "py", "pz"] {
        out.push(ready(
            "Transform",
            property,
            &format!("Position {}", axis_label(property)),
            ValueKind::Number,
            &spatial,
            Scalar,
            Translation,
            AnimationRuntimeSink::TransformProjection,
        ));
    }
    for property in ["qx", "qy", "qz", "qw"] {
        out.push(ready(
            "Transform",
            property,
            &format!("Quaternion {}", axis_label(property)),
            ValueKind::Number,
            &spatial,
            Scalar,
            Rotation,
            AnimationRuntimeSink::TransformProjection,
        ));
    }
    // ADR-172 — `rx`/`ry`/`rz` and `sx`/`sy`/`sz` used to be listed here, visible-but-disabled, with
    // the reason "these fields exist in the core schema but the native projection does not consume
    // them". They existed in the core schema and NOWHERE ELSE: no writer in this repository has ever
    // put one on a `Transform`, `local_transform` does not read them, and the projection at
    // `main.rs`'s animation sink rejects them by name. `stdlib.rs` no longer declares them, so the
    // sentence they were kept for is gone and six catalog rows that could never bind are gone with
    // it. A row that names a property nothing stores is the inert control `<ux_quality>` 6 forbids,
    // in a data table.

    for component in ["KinematicJoint", "Joint"] {
        out.push(requires_validation(
            component,
            "value",
            "Joint value",
            ValueKind::Number,
            &[ThreeD, Generic],
            Scalar,
            JointValue,
            AnimationRuntimeSink::KinematicJointProjection,
        ));
    }

    out.push(protected(
        "Sprite",
        "texture",
        "Texture",
        ValueKind::String,
        &[TwoD],
        Text,
        SpriteTexture,
        "Texture is an asset reference, not an interpolated animation value.",
    ));
    out.push(preview_only(
        "Sprite",
        "layer",
        "Layer",
        ValueKind::Integer,
        &[TwoD],
        Integer,
        SpriteLayer,
        AnimationRuntimeSink::EditorTwoDProjection,
    ));
    out.push(preview_only(
        "Sprite",
        "frame",
        "Sprite frame",
        ValueKind::Integer,
        &[TwoD],
        Integer,
        AnimationSemantic::SpriteFrame,
        AnimationRuntimeSink::EditorTwoDProjection,
    ));
    for (property, label) in [("flipX", "Flip horizontally"), ("flipY", "Flip vertically")] {
        out.push(preview_only(
            "Sprite",
            property,
            label,
            ValueKind::Boolean,
            &[TwoD],
            Toggle,
            SpriteFlip,
            AnimationRuntimeSink::EditorTwoDProjection,
        ));
    }
    out.push(preview_only(
        "Sprite",
        "opacity",
        "Opacity",
        ValueKind::Number,
        &[TwoD],
        Opacity,
        AnimationSemantic::Opacity,
        AnimationRuntimeSink::EditorTwoDProjection,
    ));
    for property in ["tintR", "tintG", "tintB", "tintA"] {
        out.push(preview_only(
            "Sprite",
            property,
            &format!("Tint {}", axis_label(property)),
            ValueKind::Number,
            &[TwoD],
            Color,
            AnimationSemantic::Color,
            AnimationRuntimeSink::EditorTwoDProjection,
        ));
    }

    out.push(preview_only(
        "HealthBar",
        "width",
        "Width",
        ValueKind::Number,
        &[Ui],
        Scalar,
        UiLayout,
        AnimationRuntimeSink::EditorUiProjection,
    ));
    out.push(preview_only(
        "HealthBar",
        "anchor",
        "Anchor",
        ValueKind::String,
        &[Ui],
        Text,
        UiAnchor,
        AnimationRuntimeSink::EditorUiProjection,
    ));

    for property in ["x", "y", "width", "height"] {
        out.push(preview_only(
            "UiStyle",
            property,
            &format!("UI {}", display_token(property)),
            ValueKind::Number,
            &[Ui],
            Scalar,
            UiLayout,
            AnimationRuntimeSink::EditorUiProjection,
        ));
    }
    out.push(preview_only(
        "UiStyle",
        "opacity",
        "Opacity",
        ValueKind::Number,
        &[Ui],
        Opacity,
        AnimationSemantic::Opacity,
        AnimationRuntimeSink::EditorUiProjection,
    ));
    out.push(preview_only(
        "UiStyle",
        "scale",
        "UI scale",
        ValueKind::Number,
        &[Ui],
        Scalar,
        AnimationSemantic::Scale,
        AnimationRuntimeSink::EditorUiProjection,
    ));
    for property in ["r", "g", "b", "a"] {
        out.push(preview_only(
            "UiStyle",
            property,
            &format!("UI color {}", axis_label(property)),
            ValueKind::Number,
            &[Ui],
            Color,
            AnimationSemantic::Color,
            AnimationRuntimeSink::EditorUiProjection,
        ));
    }
    out.push(preview_only(
        "UiStyle",
        "visible",
        "Visibility",
        ValueKind::Boolean,
        &[Ui],
        Visibility,
        AnimationSemantic::Visibility,
        AnimationRuntimeSink::EditorUiProjection,
    ));

    for (property, label, semantic) in [
        ("intensity", "Intensity", LightIntensity),
        ("range", "Range", LightRange),
        ("r", "Red", AnimationSemantic::Color),
        ("g", "Green", AnimationSemantic::Color),
        ("b", "Blue", AnimationSemantic::Color),
        ("dirX", "Direction X", LightDirection),
        ("dirY", "Direction Y", LightDirection),
        ("dirZ", "Direction Z", LightDirection),
    ] {
        out.push(unsupported(
            "Light",
            property,
            label,
            ValueKind::Number,
            &[ThreeD],
            Scalar,
            semantic,
        ));
    }
    out.push(unsupported(
        "Light",
        "kind",
        "Light type",
        ValueKind::String,
        &[ThreeD],
        Text,
        LightKind,
    ));
    out.push(unsupported(
        "Light",
        "castShadows",
        "Cast shadows",
        ValueKind::Boolean,
        &[ThreeD],
        Toggle,
        LightShadows,
    ));

    out.push(unsupported(
        "Camera",
        "fov",
        "Field of view",
        ValueKind::Number,
        &[ThreeD],
        Scalar,
        CameraFieldOfView,
    ));
    for property in ["near", "far"] {
        out.push(unsupported(
            "Camera",
            property,
            &format!("{} clip plane", display_token(property)),
            ValueKind::Number,
            &[ThreeD],
            Scalar,
            CameraClipPlane,
        ));
    }
    out.push(unsupported(
        "Camera",
        "active",
        "Active camera",
        ValueKind::Boolean,
        &[ThreeD],
        Toggle,
        CameraActive,
    ));

    out
}

#[allow(clippy::too_many_arguments)] // Flat descriptor-row helper keeps the catalog audit-visible.
fn ready(
    component: &str,
    property: &str,
    label: &str,
    value_kind: ValueKind,
    domains: &[AnimationDomain],
    editor_kind: AnimationEditorKind,
    semantic: AnimationSemantic,
    runtime_sink: AnimationRuntimeSink,
) -> AnimationBindingDescriptor {
    descriptor(
        component,
        property,
        label,
        value_kind,
        domains,
        editor_kind,
        semantic,
        Some(runtime_sink),
        AnimationReadiness::Ready,
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn preview_only(
    component: &str,
    property: &str,
    label: &str,
    value_kind: ValueKind,
    domains: &[AnimationDomain],
    editor_kind: AnimationEditorKind,
    semantic: AnimationSemantic,
    runtime_sink: AnimationRuntimeSink,
) -> AnimationBindingDescriptor {
    descriptor(
        component,
        property,
        label,
        value_kind,
        domains,
        editor_kind,
        semantic,
        Some(runtime_sink),
        AnimationReadiness::PreviewOnly,
        vec![AnimationBindingDiagnostic::new(
            Severity::Info,
            AnimationBindingDiagnosticCode::RuntimeSinkUnavailable,
            format!("{component}.{property} is available in the shared editor preview."),
            "Package a matching game runtime adapter before treating this channel as deployment-ready.",
        )],
    )
}

#[allow(clippy::too_many_arguments)] // Flat descriptor-row helper keeps the catalog audit-visible.
fn requires_validation(
    component: &str,
    property: &str,
    label: &str,
    value_kind: ValueKind,
    domains: &[AnimationDomain],
    editor_kind: AnimationEditorKind,
    semantic: AnimationSemantic,
    runtime_sink: AnimationRuntimeSink,
) -> AnimationBindingDescriptor {
    descriptor(
        component,
        property,
        label,
        value_kind,
        domains,
        editor_kind,
        semantic,
        Some(runtime_sink),
        AnimationReadiness::RequiresValidation,
        vec![AnimationBindingDiagnostic::new(
            Severity::Warning,
            AnimationBindingDiagnosticCode::TargetValidationRequired,
            "Joint animation is available only for a validated kinematic joint.",
            "Validate the selected target as KinematicJoint; physics Joint references remain protected.",
        )],
    )
}

fn unsupported(
    component: &str,
    property: &str,
    label: &str,
    value_kind: ValueKind,
    domains: &[AnimationDomain],
    editor_kind: AnimationEditorKind,
    semantic: AnimationSemantic,
) -> AnimationBindingDescriptor {
    descriptor(
        component,
        property,
        label,
        value_kind,
        domains,
        editor_kind,
        semantic,
        None,
        AnimationReadiness::Unsupported,
        vec![AnimationBindingDiagnostic::new(
            Severity::Error,
            AnimationBindingDiagnosticCode::RuntimeSinkUnavailable,
            format!("{component}.{property} has no animation runtime sink yet."),
            "Keep this property read-only until its component projection consumes evaluated values.",
        )],
    )
}

#[allow(clippy::too_many_arguments)]
fn protected(
    component: &str,
    property: &str,
    label: &str,
    value_kind: ValueKind,
    domains: &[AnimationDomain],
    editor_kind: AnimationEditorKind,
    semantic: AnimationSemantic,
    reason: &str,
) -> AnimationBindingDescriptor {
    descriptor(
        component,
        property,
        label,
        value_kind,
        domains,
        editor_kind,
        semantic,
        None,
        AnimationReadiness::Unsupported,
        vec![AnimationBindingDiagnostic::new(
            Severity::Error,
            AnimationBindingDiagnosticCode::ProtectedReference,
            reason,
            "Animate a typed presentation property instead of replacing the referenced asset.",
        )],
    )
}

#[allow(clippy::too_many_arguments)]
fn descriptor(
    component: &str,
    property: &str,
    label: &str,
    value_kind: ValueKind,
    domains: &[AnimationDomain],
    editor_kind: AnimationEditorKind,
    semantic: AnimationSemantic,
    runtime_sink: Option<AnimationRuntimeSink>,
    readiness: AnimationReadiness,
    diagnostics: Vec<AnimationBindingDiagnostic>,
) -> AnimationBindingDescriptor {
    AnimationBindingDescriptor {
        component: component.to_owned(),
        property: property.to_owned(),
        label: label.to_owned(),
        value_kind,
        domains: domains.iter().copied().collect::<BTreeSet<_>>(),
        editor_kind,
        semantic,
        runtime_sink,
        readiness,
        diagnostics,
    }
}

fn reconcile_metadata(
    mut descriptor: AnimationBindingDescriptor,
    metadata: &BTreeMap<&str, &ComponentMeta>,
) -> AnimationBindingDescriptor {
    let Some(component) = metadata.get(descriptor.component.as_str()) else {
        descriptor.diagnostics.push(AnimationBindingDiagnostic::new(
            Severity::Warning,
            AnimationBindingDiagnosticCode::ComponentMetadataMissing,
            format!(
                "{} is not registered in the core component catalog.",
                descriptor.component
            ),
            "Register component metadata so the animation catalog can verify its field type.",
        ));
        return descriptor;
    };
    let Some(field) = component
        .fields
        .iter()
        .find(|field| field.name == descriptor.property)
    else {
        descriptor.diagnostics.push(AnimationBindingDiagnostic::new(
            Severity::Warning,
            AnimationBindingDiagnosticCode::PropertyMetadataMissing,
            format!(
                "{}.{} is absent from the core component schema.",
                descriptor.component, descriptor.property
            ),
            "Align the component schema and runtime projection on one canonical field name.",
        ));
        return descriptor;
    };
    let declared = animation_kind(field.ty);
    if declared != descriptor.value_kind {
        descriptor.readiness = AnimationReadiness::Unsupported;
        descriptor.runtime_sink = None;
        descriptor.diagnostics.push(AnimationBindingDiagnostic::new(
            Severity::Error,
            AnimationBindingDiagnosticCode::PropertyTypeMismatch,
            format!(
                "{}.{} declares {declared:?}, but its animation binding expects {:?}.",
                descriptor.component, descriptor.property, descriptor.value_kind
            ),
            "Make the component schema and animation descriptor use the same value kind.",
        ));
    }
    descriptor
}

const fn animation_kind(field_type: FieldType) -> ValueKind {
    match field_type {
        FieldType::Integer => ValueKind::Integer,
        FieldType::Number => ValueKind::Number,
        FieldType::Boolean => ValueKind::Boolean,
        FieldType::String => ValueKind::String,
    }
}

fn axis_label(property: &str) -> String {
    property
        .chars()
        .last()
        .map_or_else(String::new, |axis| axis.to_ascii_uppercase().to_string())
}

fn display_token(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_diagnostic(
        descriptor: &AnimationBindingDescriptor,
        code: AnimationBindingDiagnosticCode,
    ) -> bool {
        descriptor
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code)
    }

    #[test]
    fn standard_catalog_classifies_verified_and_future_sinks_honestly() {
        let registry = AnimationBindingRegistry::standard();
        let transform = registry.descriptor("Transform", "translation").unwrap();
        assert_eq!(transform.readiness, AnimationReadiness::Ready);
        assert_eq!(
            transform.runtime_sink,
            Some(AnimationRuntimeSink::TransformProjection)
        );
        assert!(transform.is_advertised_animatable());

        let joint = registry.descriptor("KinematicJoint", "value").unwrap();
        assert_eq!(joint.readiness, AnimationReadiness::RequiresValidation);
        assert_eq!(
            joint.runtime_sink,
            Some(AnimationRuntimeSink::KinematicJointProjection)
        );
        assert!(!joint.is_advertised_animatable());

        for (component, property, sink) in [
            (
                "Sprite",
                "layer",
                AnimationRuntimeSink::EditorTwoDProjection,
            ),
            (
                "HealthBar",
                "width",
                AnimationRuntimeSink::EditorUiProjection,
            ),
            (
                "UiStyle",
                "opacity",
                AnimationRuntimeSink::EditorUiProjection,
            ),
        ] {
            let descriptor = registry.descriptor(component, property).unwrap();
            assert_eq!(descriptor.readiness, AnimationReadiness::PreviewOnly);
            assert_eq!(descriptor.runtime_sink, Some(sink));
            assert!(descriptor.is_advertised_animatable());
            assert!(has_diagnostic(
                descriptor,
                AnimationBindingDiagnosticCode::RuntimeSinkUnavailable
            ));
        }

        for (component, property) in [("Light", "intensity"), ("Camera", "fov")] {
            let descriptor = registry.descriptor(component, property).unwrap();
            assert_eq!(descriptor.readiness, AnimationReadiness::Unsupported);
            assert_eq!(descriptor.runtime_sink, None);
            assert!(has_diagnostic(
                descriptor,
                AnimationBindingDiagnosticCode::RuntimeSinkUnavailable
            ));
        }
    }

    #[test]
    fn every_advertised_property_has_a_verified_sink() {
        let registry = AnimationBindingRegistry::standard();
        assert!(registry.advertised().next().is_some());
        assert!(registry.advertised().all(|descriptor| {
            matches!(
                descriptor.readiness,
                AnimationReadiness::Ready | AnimationReadiness::PreviewOnly
            ) && descriptor.runtime_sink.is_some()
        }));
        assert!(registry
            .catalog()
            .filter(|descriptor| descriptor.readiness == AnimationReadiness::Unsupported)
            .all(|descriptor| !descriptor.diagnostics.is_empty()));
    }

    #[test]
    fn core_metadata_is_used_and_schema_drift_remains_visible() {
        let registry = AnimationBindingRegistry::standard();
        // ADR-172 — THE TWO SWAPPED. This assertion used to read `px` canonical and `x` the
        // metadata-less alias, which is exactly backwards: `x` is the field every writer writes and
        // every reader reads, and `px` was a name only the registry had heard of. Correcting
        // `stdlib.rs` moved the diagnostic to the property that actually lacks metadata, and the
        // check still proves the same thing — the registry reads the core's schema rather than
        // asserting its own.
        let canonical = registry.descriptor("Transform", "x").unwrap();
        assert!(!has_diagnostic(
            canonical,
            AnimationBindingDiagnosticCode::PropertyMetadataMissing
        ));

        let runtime_alias = registry.descriptor("Transform", "px").unwrap();
        assert_eq!(runtime_alias.readiness, AnimationReadiness::Ready);
        assert!(has_diagnostic(
            runtime_alias,
            AnimationBindingDiagnosticCode::PropertyMetadataMissing
        ));

        let bad_metadata = [ComponentMeta::builder("Transform")
            .field("x", FieldType::String, true)
            .build()];
        let mismatched = AnimationBindingRegistry::with_component_metadata(&bad_metadata);
        let descriptor = mismatched.descriptor("Transform", "x").unwrap();
        assert_eq!(descriptor.readiness, AnimationReadiness::Unsupported);
        assert_eq!(descriptor.runtime_sink, None);
        assert!(has_diagnostic(
            descriptor,
            AnimationBindingDiagnosticCode::PropertyTypeMismatch
        ));
    }

    #[test]
    fn resolve_decorates_one_property_path_and_unknown_paths_fail_closed() {
        let registry = AnimationBindingRegistry::standard();
        let path = PropertyPath::new("entity:camera", "Camera", "fov").with_subpath(["local"]);
        let context = registry.resolve(&path);
        assert_eq!(context.path, path);
        assert_eq!(context.semantic, AnimationSemantic::CameraFieldOfView);
        assert_eq!(context.readiness, AnimationReadiness::Unsupported);

        let unknown_path = PropertyPath::new("entity:hero", "Health", "hp");
        let unknown = registry.resolve(&unknown_path);
        assert_eq!(unknown.path, unknown_path);
        assert_eq!(unknown.value_kind, None);
        assert!(has_context_diagnostic(
            &unknown,
            AnimationBindingDiagnosticCode::UnknownBinding
        ));
    }

    fn has_context_diagnostic(
        context: &AnimationBindingContext,
        code: AnimationBindingDiagnosticCode,
    ) -> bool {
        context
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code)
    }

    #[test]
    fn requested_component_families_are_present_in_deterministic_catalogs() {
        let registry = AnimationBindingRegistry::standard();
        for component in [
            "Transform",
            "KinematicJoint",
            "Joint",
            "Sprite",
            "HealthBar",
            "UiStyle",
            "Light",
            "Camera",
        ] {
            assert!(
                registry.catalog_for_component(component).next().is_some(),
                "missing {component} descriptors"
            );
        }
        let keys: Vec<_> = registry
            .catalog()
            .map(|descriptor| (&descriptor.component, &descriptor.property))
            .collect();
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));

        let domains: BTreeSet<_> = registry
            .catalog()
            .flat_map(|descriptor| descriptor.domains.iter().copied())
            .collect();
        assert_eq!(
            domains,
            [
                AnimationDomain::TwoD,
                AnimationDomain::ThreeD,
                AnimationDomain::Ui,
                AnimationDomain::Generic,
            ]
            .into_iter()
            .collect()
        );
    }
}
