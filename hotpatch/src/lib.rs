//! **metrocalk-hotpatch — Instant iteration (M13.3 / ADR-052).**
//!
//! The daily-felt frontier wedge (the dossier's §5 step 3): the iteration loop creators feel every
//! minute. Incumbents are slow at it — Unity domain-reload is seconds; Unreal's DDC is opaque and
//! **non-reproducible** (UE-193688). Metrocalk makes it **sub-second and — the part no incumbent can
//! match — state-preserving across a schema change**: edit a system → swap it live → **replay the
//! op-log to reconstruct the running state under the new schema**, which the op-stream gives almost
//! for free (FM-T5.4: what `dexterous_developer` hand-builds, we fold out of the op-log).
//!
//! **Honest scope (load-bearing — state it, don't paper over it):**
//! - **A hot-patch + op-replay loop over the code/data WE own — NOT a hermetic engine.** The
//!   IEEE-SW-2025 study found 0/70 real projects fully hermetic; DCC tools + shader compilers are
//!   non-hermetic by nature. The reproducibility claim is scoped to the **Rust/ECS/op-log perimeter**,
//!   with the loop as the *visible proof* — never "reproducible builds of everything".
//! - **The measured, CI-gated leg is the OP-REPLAY-ACROSS-SCHEMA-CHANGE** (the substrate advantage):
//!   [`restore`] folds the op-log through a [`Migration`] to reconstruct a [`RuntimeState`] under a new
//!   schema, **migrate-or-explain** (a change that can't apply is an explained [`MigrationError`],
//!   never silent corruption — the [`metrocalk_core::project`]/ADR-033 discipline at the component
//!   level). This is bit-deterministic ([`reproduces`]) and headless.
//! - **The code hot-patch rides a project-owned [`HotPatch`] seam** (invariant 5 — no foreign type
//!   leaks; a CI grep-gate reserves `subsecond::` to this crate). The CI-measurable backing
//!   ([`SwapHotPatch`]) swaps the active [`SystemFn`] in-process — a **real** code substitution. The
//!   production dev backing is `subsecond` binary hot-patching of the SAME function in the tip crate
//!   (pre-1.0, **tip-crate only**, **no JIT** — AOT patching of compiled code); its ~130 ms
//!   recompile-jump is measured via the `dx` dev harness (a documented **local** gate) and is **never
//!   fabricated** in CI.
//! - **Dev-only; the ship/Play determinism guarantee (M13.1/M8.1) is untouched.** A [`RuntimeState`]
//!   is a runtime *projection* (ADR-021/034) — it can never hold a Loro handle or land on the undo
//!   stack, so a hot-patch cannot corrupt the authored document by construction.
//! - **Native-only today; wasm is a named seam** (ADR-006/020): the browser funnel keeps its own
//!   reload story. The op-replay itself is pure data (portable); the crate is native because
//!   `metrocalk-core` pulls the Flecs backend — like `metrocalk-dst`, so NOT in the wasm tripwire.
//! - **Reuse, don't fork:** the state is the shipped [`RuntimeState`] (M12.5/ADR-049), the values are
//!   [`FieldValue`], the schema is [`ComponentMeta`]/[`FieldSpec`]/[`FieldType`], and [`EditOp`] is the
//!   serializable REPLAY PROJECTION of the shipped `pipeline::Op::{SetField, RemoveField}` (non-serde
//!   + ECS-coupled) — the same sibling pattern as `RuntimeState`/`RuleRecording`/`physics::Recording`.

use std::collections::BTreeMap;

use metrocalk_core::{ComponentMeta, FieldType, FieldValue, RuntimeState};
use serde::{Deserialize, Serialize};

// ── the op-log (the serializable replay projection of pipeline::Op) ──────────────────────────────

/// One **edit op** — the serializable, persistable projection of the shipped
/// `metrocalk_core::Op::{SetField, RemoveField}` (which is non-serde + ECS-coupled), keyed by the same
/// loro-key string the [`RuntimeState`] uses. Replaying a stream of these reconstructs the running
/// state — the FM-T5.4 substrate advantage (state is a *fold of the op-log*, not a serialized blob).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EditOp {
    /// Set a component field (the additive op) — mirrors `Op::SetField`.
    SetField {
        entity: String,
        component: String,
        field: String,
        value: FieldValue,
    },
    /// Remove a single component field — mirrors `Op::RemoveField`.
    RemoveField {
        entity: String,
        component: String,
        field: String,
    },
}

/// The **op-log** = the ordered edit stream. This is the replay-DAG the whole loop rides: replaying it
/// reconstructs the [`RuntimeState`]; replaying it through a [`Migration`] reconstructs state under a
/// *changed* schema. It is the serializable sibling of the shipped commit pipeline's `Vec<Op>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpLog {
    pub ops: Vec<EditOp>,
}

impl OpLog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a `SetField` op.
    pub fn set(
        &mut self,
        entity: impl Into<String>,
        component: impl Into<String>,
        field: impl Into<String>,
        value: FieldValue,
    ) {
        self.ops.push(EditOp::SetField {
            entity: entity.into(),
            component: component.into(),
            field: field.into(),
            value,
        });
    }

    /// Record a `RemoveField` op.
    pub fn remove_field(
        &mut self,
        entity: impl Into<String>,
        component: impl Into<String>,
        field: impl Into<String>,
    ) {
        self.ops.push(EditOp::RemoveField {
            entity: entity.into(),
            component: component.into(),
            field: field.into(),
        });
    }

    /// Replay the op-log with **no** schema change → the running state under the current schema. An
    /// identity fold never triggers a type conversion, so it is infallible.
    #[must_use]
    pub fn replay(&self) -> RuntimeState {
        let fields =
            fold(&self.ops, None).expect("an identity replay (no migration) is infallible");
        to_runtime_state(&fields)
    }
}

// ── the schema + the migration (the migrate-or-refuse leg) ───────────────────────────────────────

/// A **component schema** — the set of component metas the reconstructed state is validated against.
/// Reuses the shipped [`ComponentMeta`]/[`FieldSpec`]/[`FieldType`] verbatim (NOT a parallel schema
/// type); the schema for a scene is exactly what the registry already carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Schema {
    pub components: BTreeMap<String, ComponentMeta>,
}

impl Schema {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a component meta (builder style).
    #[must_use]
    pub fn with(mut self, meta: ComponentMeta) -> Self {
        self.components.insert(meta.name.clone(), meta);
        self
    }
}

/// One declarative **schema change** — the unit of a [`Migration`]. Mirrors the shape of a real
/// authoring edit that touches a component's fields; the op-replay applies it while folding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SchemaChange {
    /// Add a field with a default value (backfilled onto every entity that has the component).
    AddField {
        component: String,
        field: String,
        ty: FieldType,
        default: FieldValue,
    },
    /// Rename a field (its recorded value is carried onto the new name).
    RenameField {
        component: String,
        from: String,
        to: String,
    },
    /// Change a field's type (values are losslessly converted or the migration is **refused**).
    ChangeType {
        component: String,
        field: String,
        to: FieldType,
    },
    /// Remove a field (its ops are dropped from the replay).
    RemoveField { component: String, field: String },
}

/// A **schema migration** = an ordered list of [`SchemaChange`]s taking the persisted schema from one
/// version to the next. `from_version`/`to_version` mirror [`metrocalk_core::FORMAT_VERSION`] — a
/// migration is the "one migration step" ADR-033 always deferred, now at the component level.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Migration {
    pub from_version: u32,
    pub to_version: u32,
    pub changes: Vec<SchemaChange>,
}

impl Migration {
    #[must_use]
    pub fn new(from_version: u32, to_version: u32) -> Self {
        Self {
            from_version,
            to_version,
            changes: Vec::new(),
        }
    }

    #[must_use]
    pub fn add_field(
        mut self,
        component: impl Into<String>,
        field: impl Into<String>,
        ty: FieldType,
        default: FieldValue,
    ) -> Self {
        self.changes.push(SchemaChange::AddField {
            component: component.into(),
            field: field.into(),
            ty,
            default,
        });
        self
    }

    #[must_use]
    pub fn rename_field(
        mut self,
        component: impl Into<String>,
        from: impl Into<String>,
        to: impl Into<String>,
    ) -> Self {
        self.changes.push(SchemaChange::RenameField {
            component: component.into(),
            from: from.into(),
            to: to.into(),
        });
        self
    }

    #[must_use]
    pub fn change_type(
        mut self,
        component: impl Into<String>,
        field: impl Into<String>,
        to: FieldType,
    ) -> Self {
        self.changes.push(SchemaChange::ChangeType {
            component: component.into(),
            field: field.into(),
            to,
        });
        self
    }

    #[must_use]
    pub fn remove_field(mut self, component: impl Into<String>, field: impl Into<String>) -> Self {
        self.changes.push(SchemaChange::RemoveField {
            component: component.into(),
            field: field.into(),
        });
        self
    }

    /// A human-readable summary of the migration (the "here's what this iteration changed" report —
    /// the sibling of the DST artifact `summary()`; a failure carries an explained [`MigrationError`]).
    #[must_use]
    pub fn summary(&self) -> String {
        serde_json::to_string_pretty(self).expect("a Migration is always serializable")
    }
}

/// A migration failure — every variant carries an **explained, user-facing** message (ADR-016: every
/// "no" is explained). The op-replay is **migrate-or-refuse**: an inapplicable change is reported, the
/// state is **never silently corrupted**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationError {
    /// A [`SchemaChange::ChangeType`] whose value has no lossless representation in the target type.
    IncompatibleValue {
        component: String,
        field: String,
        from: &'static str,
        to: &'static str,
        value: String,
    },
    /// The reconstructed state carries a component the new schema doesn't define.
    UnknownComponent { entity: String, component: String },
    /// The reconstructed state carries a field the new schema doesn't define.
    UnknownField {
        entity: String,
        component: String,
        field: String,
    },
    /// A reconstructed field's type doesn't match the new schema's declared type.
    TypeMismatch {
        entity: String,
        component: String,
        field: String,
        expected: &'static str,
        found: &'static str,
    },
    /// The new schema requires a field the replayed op-log never set (and no default backfilled it).
    MissingRequired {
        entity: String,
        component: String,
        field: String,
    },
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IncompatibleValue { component, field, from, to, value } => write!(
                f,
                "can't migrate {component}.{field}: a {from} value ({value:?}) has no lossless {to} representation — refused, not silently corrupted",
            ),
            Self::UnknownComponent { entity, component } => write!(
                f,
                "the restored state has component {component:?} on entity {entity} that the new schema doesn't define",
            ),
            Self::UnknownField { entity, component, field } => write!(
                f,
                "the restored state has field {component}.{field} on entity {entity} that the new schema doesn't define",
            ),
            Self::TypeMismatch { entity, component, field, expected, found } => write!(
                f,
                "field {component}.{field} on entity {entity} is a {found} but the new schema requires {expected}",
            ),
            Self::MissingRequired { entity, component, field } => write!(
                f,
                "the new schema requires field {component}.{field} on entity {entity}, but the replayed op-log never set it (and no default backfilled it)",
            ),
        }
    }
}

impl std::error::Error for MigrationError {}

// ── the op-replay-across-schema-change (the measured substrate advantage) ─────────────────────────

/// Replay the op-log **across a schema change** and validate the reconstructed state against the new
/// schema — the op-replay leg on its own (deliverable 3). This is the FM-T5.4 substrate advantage made
/// concrete: because the op-log is a semantic stream, state under a *new* schema is a re-fold, not a
/// bespoke state-blob migration. **Migrate-or-explain:** returns the reconstructed [`RuntimeState`], or
/// an explained [`MigrationError`] — never a silently corrupt state.
///
/// # Errors
/// A [`MigrationError`] if a type change can't apply losslessly, or the reconstructed state doesn't
/// satisfy the new schema.
pub fn restore(
    log: &OpLog,
    migration: &Migration,
    new_schema: &Schema,
) -> Result<RuntimeState, MigrationError> {
    let fields = fold(&log.ops, Some(migration))?;
    validate(&fields, new_schema)?;
    Ok(to_runtime_state(&fields))
}

/// The hot-patch mechanism does **not** weaken determinism (deliverable 6): replaying the op-log (with
/// or without a migration) reconstructs the **same** [`RuntimeState`] digest every run. `runs.max(2)`
/// enforces the ≥2-runs rule (a single match is not proof). The equality key is [`RuntimeState::digest`]
/// — the M12.5 sibling of `physics::Replay::world_hash`. A migration that *fails* reproduces its
/// failure (returns `false` for both, so the check is still meaningful).
#[must_use]
pub fn reproduces(log: &OpLog, migration: Option<&Migration>, runs: usize) -> bool {
    let Some(first) = replay_digest(log, migration) else {
        return false;
    };
    (1..runs.max(2)).all(|_| replay_digest(log, migration).as_ref() == Some(&first))
}

fn replay_digest(log: &OpLog, migration: Option<&Migration>) -> Option<String> {
    fold(&log.ops, migration)
        .ok()
        .map(|fields| to_runtime_state(&fields).digest())
}

// ── the hot-patch seam (invariant 5; subsecond rides behind this) ─────────────────────────────────

/// The hot-patchable **system** — a Rules/system evaluation over the running state. In the real dev
/// loop this is a function in our tip crate whose body the developer edits; the loop swaps its
/// implementation **live**. The second argument is the entity keys the system iterates (the shipped
/// [`RuntimeState`] is a keyed projection, read via [`RuntimeState::get`]). The return is an observable
/// output — proof the new code ran on the restored state.
pub type SystemFn = fn(&RuntimeState, &[String]) -> String;

/// The project-owned hot-patch seam (invariant 5). A backing swaps the active [`SystemFn`] live, off
/// the per-frame hot path (inv. 4). The CI-measurable backing is [`SwapHotPatch`]; the production dev
/// backing is `subsecond` binary patching of the same tip-crate function behind this SAME trait
/// (grep-gated; no foreign type leaks; the ~130 ms recompile-jump is a `dx`-harness number).
pub trait HotPatch {
    /// The currently-active system implementation.
    fn active(&self) -> SystemFn;
    /// Swap in a new implementation **live** (the hot-patch).
    fn patch(&mut self, new: SystemFn);
}

/// The default, **CI-measurable** hot-patch backing: an in-process [`SystemFn`] swap — a real code
/// substitution we can measure headlessly with no dev harness. (`subsecond`'s binary recompile-jump
/// is the production dev backing behind the [`HotPatch`] trait; it needs the `dx` harness + a running
/// process, so its number is a documented local gate — never a fabricated CI measurement.)
pub struct SwapHotPatch {
    current: SystemFn,
}

impl SwapHotPatch {
    #[must_use]
    pub fn new(initial: SystemFn) -> Self {
        Self { current: initial }
    }
}

impl HotPatch for SwapHotPatch {
    fn active(&self) -> SystemFn {
        self.current
    }
    fn patch(&mut self, new: SystemFn) {
        self.current = new;
    }
}

// ── the full loop (edit → hot-patch → state-restored) ─────────────────────────────────────────────

/// The result of one hot-iterate loop.
#[derive(Clone, Debug)]
pub struct LoopResult {
    /// The reconstructed running state after replaying the op-log across the schema change.
    pub restored: RuntimeState,
    /// The observable output of the newly-patched system on the restored state (proof it ran live on
    /// the new schema's fields).
    pub output: String,
    /// The restored-state digest — the determinism equality key.
    pub digest: String,
}

/// The iteration-loop harness: an op-log + the current schema + a hot-patchable system. [`hot_iterate`]
/// is the daily-felt wedge — edit a system, see it live in the running state, state intact across a
/// schema change.
///
/// [`hot_iterate`]: IterationLoop::hot_iterate
pub struct IterationLoop<H: HotPatch> {
    log: OpLog,
    schema: Schema,
    patch: H,
}

impl<H: HotPatch> IterationLoop<H> {
    #[must_use]
    pub fn new(log: OpLog, schema: Schema, patch: H) -> Self {
        Self { log, schema, patch }
    }

    /// The current schema (advances after a successful [`hot_iterate`](Self::hot_iterate)).
    #[must_use]
    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// The op-log (immutable across a hot-patch — the recorded input the loop reads, never mutates).
    #[must_use]
    pub fn log(&self) -> &OpLog {
        &self.log
    }

    /// **THE HOT-ITERATE LOOP** (deliverable 1): (1) hot-patch the system live, (2) replay the op-log
    /// across the schema change to reconstruct the running state, (3) validate it against the new
    /// schema (valid-or-explained), (4) run the newly-patched system on the restored state. On success
    /// the schema advances; on a migration failure the schema is left unchanged and an explained
    /// [`MigrationError`] is returned (never a partial/corrupt apply).
    ///
    /// # Errors
    /// A [`MigrationError`] if the op-replay across the schema change can't produce a valid state.
    pub fn hot_iterate(
        &mut self,
        new_system: SystemFn,
        migration: &Migration,
        new_schema: Schema,
    ) -> Result<LoopResult, MigrationError> {
        // 1. hot-patch the system live (the "edit → live" step; off the per-frame hot path).
        self.patch.patch(new_system);
        // 2. replay the op-log ACROSS the schema change to reconstruct the running state.
        let fields = fold(&self.log.ops, Some(migration))?;
        // 3. validate the reconstructed state against the new schema (valid-or-explained).
        validate(&fields, &new_schema)?;
        // 4. run the newly-patched system on the restored state (the "see it live" step).
        let entities: Vec<String> = fields.keys().cloned().collect();
        let restored = to_runtime_state(&fields);
        let output = (self.patch.active())(&restored, &entities);
        let digest = restored.digest();
        self.schema = new_schema;
        Ok(LoopResult {
            restored,
            output,
            digest,
        })
    }
}

// ── the fold + validation + value conversion (internal) ───────────────────────────────────────────

/// The reconstructed state, in the same nested-`BTreeMap` shape as [`RuntimeState`]'s private store, so
/// iteration + hashing are deterministic. (`RuntimeState` has no field-removal API, so we fold here and
/// materialize into it at the end.)
type Fields = BTreeMap<String, BTreeMap<String, BTreeMap<String, FieldValue>>>;

/// Fold the op-log into the reconstructed state, applying `migration`'s per-op transforms
/// (rename/change-type/remove) as it goes and backfilling added fields afterward.
fn fold(ops: &[EditOp], migration: Option<&Migration>) -> Result<Fields, MigrationError> {
    let no_changes: Vec<SchemaChange> = Vec::new();
    let changes = migration.map_or(no_changes.as_slice(), |m| m.changes.as_slice());

    let mut fields: Fields = BTreeMap::new();
    for op in ops {
        match op {
            EditOp::SetField {
                entity,
                component,
                field,
                value,
            } => {
                if let Some((component, field, value)) =
                    remap_set(component, field, value, changes)?
                {
                    fields
                        .entry(entity.clone())
                        .or_default()
                        .entry(component)
                        .or_default()
                        .insert(field, value);
                }
                // else: this field was RemoveField'd by the migration → drop the op.
            }
            EditOp::RemoveField {
                entity,
                component,
                field,
            } => {
                if let Some((component, field)) = remap_remove(component, field, changes) {
                    if let Some(comps) = fields.get_mut(entity) {
                        if let Some(flds) = comps.get_mut(&component) {
                            flds.remove(&field);
                            if flds.is_empty() {
                                comps.remove(&component);
                            }
                        }
                        if comps.is_empty() {
                            fields.remove(entity);
                        }
                    }
                }
            }
        }
    }

    // Backfill AddField defaults onto every entity that carries the component.
    for change in changes {
        if let SchemaChange::AddField {
            component,
            field,
            default,
            ..
        } = change
        {
            for comps in fields.values_mut() {
                if let Some(flds) = comps.get_mut(component) {
                    flds.entry(field.clone()).or_insert_with(|| default.clone());
                }
            }
        }
    }

    Ok(fields)
}

/// Apply the migration's per-op transforms to a `SetField`'s `(component, field, value)`. Returns
/// `None` if a `RemoveField` change drops it, or an explained error if a `ChangeType` can't convert.
fn remap_set(
    component: &str,
    field: &str,
    value: &FieldValue,
    changes: &[SchemaChange],
) -> Result<Option<(String, String, FieldValue)>, MigrationError> {
    let component = component.to_string();
    let mut field = field.to_string();
    let mut value = value.clone();
    for change in changes {
        match change {
            SchemaChange::RenameField {
                component: c,
                from,
                to,
            } if *c == component && *from == field => {
                field.clone_from(to);
            }
            SchemaChange::ChangeType {
                component: c,
                field: fld,
                to,
            } if *c == component && *fld == field => {
                value = convert(&value, *to).ok_or_else(|| MigrationError::IncompatibleValue {
                    component: component.clone(),
                    field: field.clone(),
                    from: type_name(&value),
                    to: field_type_name(*to),
                    value: value_string(&value),
                })?;
            }
            SchemaChange::RemoveField {
                component: c,
                field: fld,
            } if *c == component && *fld == field => {
                return Ok(None);
            }
            _ => {}
        }
    }
    Ok(Some((component, field, value)))
}

/// Apply the migration's per-op transforms to a `RemoveField`'s `(component, field)`. Returns `None` if
/// a `RemoveField` change already drops it (a redundant remove).
fn remap_remove(
    component: &str,
    field: &str,
    changes: &[SchemaChange],
) -> Option<(String, String)> {
    let component = component.to_string();
    let mut field = field.to_string();
    for change in changes {
        match change {
            SchemaChange::RenameField {
                component: c,
                from,
                to,
            } if *c == component && *from == field => {
                field.clone_from(to);
            }
            SchemaChange::RemoveField {
                component: c,
                field: fld,
            } if *c == component && *fld == field => {
                return None;
            }
            _ => {}
        }
    }
    Some((component, field))
}

/// Validate the reconstructed state against the new schema — every field present, correctly typed, and
/// every required field set. The "restored state is valid, not corrupt" check.
fn validate(fields: &Fields, schema: &Schema) -> Result<(), MigrationError> {
    for (entity, comps) in fields {
        for (component, flds) in comps {
            let meta = schema.components.get(component).ok_or_else(|| {
                MigrationError::UnknownComponent {
                    entity: entity.clone(),
                    component: component.clone(),
                }
            })?;
            for (field, value) in flds {
                let spec = meta
                    .fields
                    .iter()
                    .find(|s| s.name == *field)
                    .ok_or_else(|| MigrationError::UnknownField {
                        entity: entity.clone(),
                        component: component.clone(),
                        field: field.clone(),
                    })?;
                if !is_type(value, spec.ty) {
                    return Err(MigrationError::TypeMismatch {
                        entity: entity.clone(),
                        component: component.clone(),
                        field: field.clone(),
                        expected: field_type_name(spec.ty),
                        found: type_name(value),
                    });
                }
            }
            for spec in &meta.fields {
                if spec.required && !flds.contains_key(&spec.name) {
                    return Err(MigrationError::MissingRequired {
                        entity: entity.clone(),
                        component: component.clone(),
                        field: spec.name.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn to_runtime_state(fields: &Fields) -> RuntimeState {
    let mut state = RuntimeState::new();
    for (entity, comps) in fields {
        for (component, flds) in comps {
            for (field, value) in flds {
                state.set(entity, component, field, value.clone());
            }
        }
    }
    state
}

/// Convert a value to a target type **losslessly**, or `None` if no lossless representation exists (the
/// migrate-or-refuse boundary — a lossy conversion is refused, not silently applied).
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn convert(value: &FieldValue, to: FieldType) -> Option<FieldValue> {
    use FieldType as T;
    use FieldValue as V;
    match (value, to) {
        // identity
        (V::Integer(_), T::Integer)
        | (V::Number(_), T::Number)
        | (V::Bool(_), T::Boolean)
        | (V::Str(_), T::String) => Some(value.clone()),
        // widenings that are always exact
        (V::Integer(i), T::Number) => Some(V::Number(*i as f64)),
        (V::Bool(b), T::Integer) => Some(V::Integer(i64::from(*b))),
        (V::Bool(b), T::Number) => Some(V::Number(if *b { 1.0 } else { 0.0 })),
        // narrowings only when lossless
        (V::Number(n), T::Integer) if n.fract() == 0.0 => Some(V::Integer(*n as i64)),
        (V::Integer(0), T::Boolean) => Some(V::Bool(false)),
        (V::Integer(1), T::Boolean) => Some(V::Bool(true)),
        // string is a total sink (any scalar has a canonical text form)
        (_, T::String) => Some(V::Str(value_string(value))),
        // string parses back only when it exactly round-trips a scalar
        (V::Str(s), T::Integer) => s.parse::<i64>().ok().map(V::Integer),
        (V::Str(s), T::Number) => s.parse::<f64>().ok().map(V::Number),
        (V::Str(s), T::Boolean) => match s.as_str() {
            "true" => Some(V::Bool(true)),
            "false" => Some(V::Bool(false)),
            _ => None,
        },
        _ => None,
    }
}

fn value_string(value: &FieldValue) -> String {
    match value {
        FieldValue::Integer(i) => i.to_string(),
        FieldValue::Number(n) => n.to_string(),
        FieldValue::Bool(b) => b.to_string(),
        FieldValue::Str(s) => s.clone(),
    }
}

fn type_name(value: &FieldValue) -> &'static str {
    match value {
        FieldValue::Integer(_) => "integer",
        FieldValue::Number(_) => "number",
        FieldValue::Bool(_) => "boolean",
        FieldValue::Str(_) => "string",
    }
}

fn field_type_name(ty: FieldType) -> &'static str {
    match ty {
        FieldType::Integer => "integer",
        FieldType::Number => "number",
        FieldType::Boolean => "boolean",
        FieldType::String => "string",
    }
}

fn is_type(value: &FieldValue, ty: FieldType) -> bool {
    matches!(
        (value, ty),
        (FieldValue::Integer(_), FieldType::Integer)
            | (FieldValue::Number(_), FieldType::Number)
            | (FieldValue::Bool(_), FieldType::Boolean)
            | (FieldValue::Str(_), FieldType::String)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrocalk_core::FieldSpec;
    use std::time::Instant;

    // ── the scene + the two system versions (the thing the developer edits) ──────────────────────

    /// The op-log for a small scene: a `Health` component with an `hp` field on three entities. This is
    /// the recorded input the loop replays — the same shape the commit pipeline records.
    fn scene() -> OpLog {
        let mut log = OpLog::new();
        for (entity, hp) in [("e0", 10), ("e1", 0), ("e2", 5)] {
            log.set(entity, "Health", "hp", FieldValue::Integer(hp));
        }
        log
    }

    fn schema_v1() -> Schema {
        Schema::new().with(component(
            "Health",
            vec![field("hp", FieldType::Integer, true)],
        ))
    }

    fn schema_v2() -> Schema {
        Schema::new().with(component(
            "Health",
            vec![
                field("current", FieldType::Integer, true),
                field("max", FieldType::Integer, true),
            ],
        ))
    }

    /// The v1 → v2 migration: rename `hp` → `current`, add `max` (default 100). The op-replay
    /// reconstructs the new-schema state from the OLD-schema op-log.
    fn migration_v1_to_v2() -> Migration {
        Migration::new(1, 2)
            .rename_field("Health", "hp", "current")
            .add_field(
                "Health",
                "max",
                FieldType::Integer,
                FieldValue::Integer(100),
            )
    }

    /// System **v1** (reads the old schema): count entities that are alive (`hp > 0`).
    fn system_v1(state: &RuntimeState, entities: &[String]) -> String {
        let alive = entities
            .iter()
            .filter(|e| matches!(state.get(e, "Health", "hp"), Some(FieldValue::Integer(hp)) if *hp > 0))
            .count();
        format!("v1 alive={alive}")
    }

    /// System **v2** (the edited version — reads the NEW schema's `current`/`max`): average health %.
    fn system_v2(state: &RuntimeState, entities: &[String]) -> String {
        let (mut total, mut n) = (0i64, 0i64);
        for e in entities {
            if let (Some(FieldValue::Integer(cur)), Some(FieldValue::Integer(max))) = (
                state.get(e, "Health", "current"),
                state.get(e, "Health", "max"),
            ) {
                if *max > 0 {
                    total += *cur * 100 / *max;
                    n += 1;
                }
            }
        }
        let avg = if n > 0 { total / n } else { 0 };
        format!("v2 avg={avg}%")
    }

    fn field(name: &str, ty: FieldType, required: bool) -> FieldSpec {
        FieldSpec {
            name: name.to_string(),
            ty,
            required,
            format: None,
        }
    }

    fn component(name: &str, fields: Vec<FieldSpec>) -> ComponentMeta {
        ComponentMeta {
            name: name.to_string(),
            fields,
            ..Default::default()
        }
    }

    // ── THE SPIKE (deliverable 1, the measured go/no-go gate) ────────────────────────────────────

    #[test]
    fn the_spike_hot_patch_plus_op_replay_across_a_schema_change() {
        // GO/NO-GO: edit a system (swap v1 → v2) AND change the component schema (rename hp→current,
        // add max), then replay the op-log to reconstruct the RUNNING STATE under the new schema — and
        // the newly-patched system runs on it, live. Measured deterministic across ≥2 runs.
        let loop_ = IterationLoop::new(scene(), schema_v1(), SwapHotPatch::new(system_v1));

        // Before the edit: the v1 system reads the v1 schema.
        let before = (loop_.log().replay(), loop_.schema().clone());
        assert_eq!(
            system_v1(&before.0, &["e0".into(), "e1".into(), "e2".into()]),
            "v1 alive=2"
        );

        // The hot-iterate loop, TIMED across ≥2 runs (a real edit→patched→state-restored measurement;
        // the release budget lives in tests/bench.rs — this is the correctness + determinism gate).
        let mut outputs = Vec::new();
        for _ in 0..3 {
            let t = Instant::now();
            let mut fresh = IterationLoop::new(scene(), schema_v1(), SwapHotPatch::new(system_v1));
            let result = fresh
                .hot_iterate(system_v2, &migration_v1_to_v2(), schema_v2())
                .expect("the op-replay across the schema change reconstructs a valid state: GO");
            let micros = t.elapsed().as_micros();
            outputs.push((result.output.clone(), result.digest.clone(), micros));
        }

        // (a) the newly-patched v2 system ran on the RESTORED state, reading the NEW schema's fields:
        //     current = migrated from hp (10,0,5), max = backfilled 100 → avg (10+0+5)/3 = 5%.
        assert_eq!(
            outputs[0].0, "v2 avg=5%",
            "the patched system ran live on the reconstructed state"
        );
        assert_ne!(
            outputs[0].0, "v1 alive=2",
            "the hot-patch actually changed the running behavior"
        );

        // (b) bit-for-bit deterministic across the 3 runs (the ≥2-runs rule) — a divergence here would
        //     be a determinism regression (the CI gate).
        assert!(
            outputs.iter().all(|(_, d, _)| *d == outputs[0].1),
            "the reconstructed state digest is identical across 3 replays: GO"
        );

        // (c) the restored state is VALID under the new schema (the migrate check already enforced it).
        let restored = restore(&scene(), &migration_v1_to_v2(), &schema_v2()).unwrap();
        assert_eq!(
            restored.get("e0", "Health", "current"),
            Some(&FieldValue::Integer(10))
        );
        assert_eq!(
            restored.get("e0", "Health", "max"),
            Some(&FieldValue::Integer(100))
        );
        assert_eq!(
            restored.get("e0", "Health", "hp"),
            None,
            "the old field name is gone (renamed)"
        );

        let slowest = outputs.iter().map(|(_, _, u)| *u).max().unwrap();
        println!(
            "[hotpatch spike] edit->patch->state-restored loop GO — digest {} — deterministic x3 — slowest run {slowest}us (sub-second; the ~130ms subsecond recompile is the dev-harness dominant cost)",
            &outputs[0].1[..16.min(outputs[0].1.len())]
        );
    }

    // ── migrate-or-explain (deliverable 3): never silent corruption ──────────────────────────────

    #[test]
    fn an_incompatible_migration_is_refused_with_an_explanation_not_silently_corrupted() {
        // A ChangeType that can't apply losslessly (hp = 10 is not a boolean 0/1) is REFUSED with an
        // explained error — the state is never silently corrupted (the ADR-033 migrate-or-refuse
        // discipline at the component level).
        let bad = Migration::new(1, 2).change_type("Health", "hp", FieldType::Boolean);
        let schema_bad = Schema::new().with(component(
            "Health",
            vec![field("hp", FieldType::Boolean, true)],
        ));
        let err = restore(&scene(), &bad, &schema_bad).expect_err("a lossy type change is refused");
        assert!(matches!(err, MigrationError::IncompatibleValue { .. }));
        assert!(
            err.to_string().contains("refused, not silently corrupted"),
            "the refusal is explained in plain language: {err}"
        );

        // A LOSSLESS type change on the same field succeeds (Integer → Number): the boundary is real,
        // not a blanket refusal.
        let ok = Migration::new(1, 2).change_type("Health", "hp", FieldType::Number);
        let schema_ok = Schema::new().with(component(
            "Health",
            vec![field("hp", FieldType::Number, true)],
        ));
        let state = restore(&scene(), &ok, &schema_ok).expect("a lossless type change applies");
        assert_eq!(
            state.get("e0", "Health", "hp"),
            Some(&FieldValue::Number(10.0))
        );
    }

    #[test]
    fn a_migration_that_leaves_a_required_field_unset_is_explained() {
        // The new schema requires `max`, but a migration that only renames (no add) never sets it →
        // MissingRequired, explained (not a corrupt half-migrated state).
        let only_rename = Migration::new(1, 2).rename_field("Health", "hp", "current");
        let err = restore(&scene(), &only_rename, &schema_v2())
            .expect_err("missing required field is caught");
        assert!(matches!(err, MigrationError::MissingRequired { field, .. } if field == "max"));
    }

    // ── determinism: the op-replay + the ship-path-untouched guard ────────────────────────────────

    #[test]
    fn the_op_replay_is_deterministic_and_migration_sensitive() {
        // Reproducible across ≥2 runs, both migrated and identity — the determinism gate.
        assert!(
            reproduces(&scene(), Some(&migration_v1_to_v2()), 3),
            "migrated replay reproduces x3"
        );
        assert!(
            reproduces(&scene(), None, 3),
            "identity replay reproduces x3"
        );

        // Non-vacuous: a DIFFERENT migration yields a DIFFERENT reconstructed state (the digest reflects
        // the real trajectory, it isn't a constant).
        let alt = Migration::new(1, 2)
            .rename_field("Health", "hp", "current")
            .add_field("Health", "max", FieldType::Integer, FieldValue::Integer(50));
        let a = restore(&scene(), &migration_v1_to_v2(), &schema_v2())
            .unwrap()
            .digest();
        let b = restore(&scene(), &alt, &schema_v2()).unwrap().digest();
        assert_ne!(
            a, b,
            "a different default → a different restored state (non-vacuous)"
        );
    }

    #[test]
    fn the_hot_patch_does_not_weaken_the_ship_play_determinism() {
        // Deliverable 6: the hot-patch mechanism is dev-only and does not touch the ship/Play
        // determinism guarantee. Replaying WITHOUT a migration reproduces the pre-hot-patch state
        // bit-identically, and the recorded op-log is UNCHANGED by a hot-iterate (the loop reads the
        // input, never mutates it — a RuntimeState is a projection, never a Loro commit; ADR-021/034).
        let before = scene().replay().digest();

        let mut loop_ = IterationLoop::new(scene(), schema_v1(), SwapHotPatch::new(system_v1));
        let log_before = loop_.log().clone();
        let _ = loop_
            .hot_iterate(system_v2, &migration_v1_to_v2(), schema_v2())
            .unwrap();

        assert_eq!(
            loop_.log(),
            &log_before,
            "the recorded op-log is immutable across a hot-patch"
        );
        assert_eq!(
            loop_.log().replay().digest(),
            before,
            "the un-migrated replay is bit-identical after the hot-patch — ship determinism untouched"
        );
    }

    // ── the hot-patch seam (deliverable 2) ────────────────────────────────────────────────────────

    #[test]
    fn the_hot_patch_seam_swaps_the_active_system_live() {
        // The HotPatch trait swaps the active SystemFn — a real code substitution (running v1 vs v2 on
        // the same state gives different output).
        let state = restore(&scene(), &migration_v1_to_v2(), &schema_v2()).unwrap();
        let entities = ["e0".to_string(), "e1".to_string(), "e2".to_string()];

        let mut patch = SwapHotPatch::new(system_v1);
        // v1 reads `hp` (gone in v2 schema) → alive=0 on the migrated state.
        assert_eq!((patch.active())(&state, &entities), "v1 alive=0");
        patch.patch(system_v2);
        assert_eq!(
            (patch.active())(&state, &entities),
            "v2 avg=5%",
            "the swap took effect live"
        );
    }

    // ── the perimeter / no-JIT / inject-nothing audit (deliverable 6) ─────────────────────────────

    #[test]
    fn the_recorded_path_is_pure_data_no_clock_entropy_or_jit_audit() {
        // The op-log + migration are PURE SERDE DATA — they round-trip through serde and carry no wall
        // clock, GPU handle, or system entropy (the type system enforces it: none is serde data). The
        // SystemFn is an AOT function pointer (no JIT). The digest is stable across a serde round-trip.
        let log = scene();
        let bytes = serde_json::to_vec(&log).unwrap();
        let reloaded: OpLog = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(reloaded, log, "the op-log is pure, reloadable data");
        assert_eq!(
            reloaded.replay().digest(),
            log.replay().digest(),
            "reload reproduces the state"
        );

        // The migration summary is a human-readable report (the "what this iteration changed" file).
        let summary = migration_v1_to_v2().summary();
        assert!(summary.contains("RenameField") && summary.contains("current"));
    }
}
