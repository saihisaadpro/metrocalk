//! Component metadata registry — the data layer behind the compatibility query and (later)
//! describe-to-create. A component *kind* registers its field schema and its relational
//! capabilities (provides / requires / observes); the registry interns each capability as an
//! [`Entity`] and records the kind as an entity carrying `(Provides, cap)` / `(Requires, cap)` /
//! `(Observes, cap)` pairs, so "what provides Health?" is answered through the M1.2 [`World`]
//! query wrapper — never reaching behind it.
//!
//! It is generic over `W: World`, so the same registry data drives the native Flecs backend today
//! and the Phase-2 pure-Rust-over-Loro backend (ADR-006), and is the same metadata a marketplace
//! component (ADR-004) registers through.

use metrocalk_ecs::{Clause, Entity, Target, Term, World};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use thiserror::Error;

/// A scalar field type (the subset of JSON-Schema types component fields use).
#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Copy, Debug)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    /// JSON Schema `integer` → `i64`.
    Integer,
    /// JSON Schema `number` → `f64`.
    Number,
    /// JSON Schema `boolean`.
    Boolean,
    /// JSON Schema `string` (asset refs are `string` + `format: "asset"`).
    String,
}

impl FieldType {
    fn from_schema(s: &str) -> Option<Self> {
        match s {
            "integer" => Some(Self::Integer),
            "number" => Some(Self::Number),
            "boolean" => Some(Self::Boolean),
            "string" => Some(Self::String),
            _ => None,
        }
    }
}

/// One field of a component's schema.
#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Debug)]
pub struct FieldSpec {
    /// Field name.
    pub name: String,
    /// Scalar type.
    pub ty: FieldType,
    /// Whether the schema marks the field required.
    pub required: bool,
    /// Optional JSON-Schema `format` (e.g. `"asset"`, `"color"`) — a semantic/UI hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// A component kind's full metadata — the unit of registration, serialization, and (later) the
/// marketplace catalog entry. Field order is canonical (sorted by name) so round-trips are stable.
#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Debug, Default)]
pub struct ComponentMeta {
    /// Unique kind name, e.g. `"Health"`.
    pub name: String,
    /// Schema fields, sorted by name.
    pub fields: Vec<FieldSpec>,
    /// Capabilities this kind provides (e.g. `Health` provides `"Health"`).
    pub provides: Vec<String>,
    /// Capabilities this kind requires to bind (e.g. `HealthBar` requires `"Health"`).
    pub requires: Vec<String>,
    /// Capabilities this kind observes for live updates.
    pub observes: Vec<String>,
    /// Describe-to-create search tags (search itself is later — ADR-004).
    pub tags: Vec<String>,
    /// Alternate names for search.
    pub aliases: Vec<String>,
    /// Optional per-field UI/semantic hints (field name → hint), kept sorted for stable round-trips.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ui_hints: BTreeMap<String, String>,
    /// The catalog category this kind browses under (M3.4, "+ Add" palette) — **canonical** (`std:UI`);
    /// `None` = uncategorized (groups under `std:Other`). See [`crate::taxonomy`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

impl ComponentMeta {
    /// Start building a component kind.
    pub fn builder(name: impl Into<String>) -> Builder {
        Builder {
            meta: ComponentMeta {
                name: name.into(),
                ..Default::default()
            },
        }
    }

    /// Serialize to a JSON string (deterministic — stable field order, so round-trips are byte-identical).
    pub fn to_json(&self) -> String {
        // Serialization of these plain types is infallible.
        serde_json::to_string(self).expect("ComponentMeta is always serializable")
    }

    /// Deserialize from a JSON string (the marketplace/persistence path — ADR-004).
    ///
    /// # Errors
    /// [`RegistryError::InvalidJson`] if the JSON is invalid or doesn't match the schema.
    pub fn from_json(s: &str) -> Result<Self, RegistryError> {
        serde_json::from_str(s).map_err(|e| RegistryError::InvalidJson(e.to_string()))
    }
}

/// A rule **event** the registry knows — the "When" vocabulary the M12.1 Rules builder offers (ADR-045).
/// A name + a plain-language description (the typo-proof dropdown source + its tooltip). Event *payloads*
/// (the subject entity a fired event carries) are the M12.5 runtime concern; M12.1 is the authoring model.
#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Debug)]
pub struct EventMeta {
    /// Unique event name, e.g. `"EnemyDied"`.
    pub name: String,
    /// Plain-language description (no jargon — the UX-quality "scent" bar).
    pub description: String,
}

impl EventMeta {
    /// Build an event-meta from a name + description.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }
}

/// A rule **action** verb the registry knows — the "Then" vocabulary (ADR-045). The action set is
/// **closed** (the honest ceiling, test #5): verbs over component fields, never free code — genuinely
/// algorithmic behaviour is the M12.3 WASM-plugin tier.
#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Debug)]
pub struct ActionMeta {
    /// Unique action name, e.g. `"SetField"`.
    pub name: String,
    /// Plain-language description of what the verb does.
    pub description: String,
}

impl ActionMeta {
    /// Build an action-meta from a name + description.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }
}

/// A **WASM plugin component** the registry knows (M12.3 / ADR-047) — the *honest ceiling*: genuinely
/// algorithmic behaviour lives in a sandboxed plugin, not in Rules. Registering it makes the plugin
/// **referenceable + typed** so an M12.1 Rule's `RunPlugin` action can name it (reveal/explain applies) and
/// the host loads it by name. The `deterministic` flag is the **Play/replay gate** (deliverable 6): a
/// non-deterministic plugin can't enter the lockstep path. The registry knows the plugin's name + contract;
/// the sandboxed *running* lives in `/plugins` (invariant 5).
#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Debug)]
pub struct PluginMeta {
    /// Unique plugin name, e.g. `"arrange"` (what a `RunPlugin` action references + the host loads by).
    pub name: String,
    /// Plain-language description of what the plugin computes (the explain/scent surface).
    pub description: String,
    /// Whether the plugin is **deterministic** (same input → same output). Only a deterministic plugin may
    /// run in the Play/replay lockstep path (M8.1); a non-deterministic one is flagged out of it.
    pub deterministic: bool,
}

impl PluginMeta {
    /// Build a plugin-meta from a name + description + the determinism flag.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        deterministic: bool,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            deterministic,
        }
    }
}

/// Ergonomic, explicit builder for a [`ComponentMeta`] (chosen over a derive macro — less magic, and
/// it works for runtime/plugin/marketplace registration, not only compile-time types).
#[derive(Debug)]
pub struct Builder {
    meta: ComponentMeta,
}

// Chaining builder: methods return `Self`; the terminal `build()` (-> ComponentMeta) is what's used.
#[allow(clippy::return_self_not_must_use)]
impl Builder {
    /// Add one field directly.
    pub fn field(self, name: impl Into<String>, ty: FieldType, required: bool) -> Self {
        self.field_fmt(name, ty, required, None::<String>)
    }

    /// Add one field with an optional `format` (e.g. `"asset"`, `"color"`).
    pub fn field_fmt(
        mut self,
        name: impl Into<String>,
        ty: FieldType,
        required: bool,
        format: Option<impl Into<String>>,
    ) -> Self {
        self.meta.fields.push(FieldSpec {
            name: name.into(),
            ty,
            required,
            format: format.map(Into::into),
        });
        self
    }

    /// Populate fields (and per-field UI hints from `description`) from a JSON Schema.
    ///
    /// Accepts the subset components use: a root `{"type": "object", "properties": { … }}` whose
    /// properties have a scalar `"type"` (`integer`/`number`/`boolean`/`string`), an optional
    /// `"format"`/`"description"`, and an optional top-level `"required": [..]`.
    ///
    /// # Errors
    /// Returns a [`RegistryError`] with an actionable message if the schema is malformed.
    pub fn fields_from_json_schema(mut self, schema: &Value) -> Result<Self, RegistryError> {
        let (fields, hints) = parse_schema(schema)?;
        self.meta.fields = fields;
        self.meta.ui_hints.extend(hints);
        Ok(self)
    }

    /// Like [`fields_from_json_schema`](Self::fields_from_json_schema) but from a JSON string — the
    /// path a marketplace/plugin component (a JSON document) registers through (ADR-004).
    ///
    /// # Errors
    /// [`RegistryError::InvalidJson`] if the text isn't valid JSON, else any schema error.
    pub fn fields_from_json_str(self, schema: &str) -> Result<Self, RegistryError> {
        let v: Value =
            serde_json::from_str(schema).map_err(|e| RegistryError::InvalidJson(e.to_string()))?;
        self.fields_from_json_schema(&v)
    }

    /// Declare a provided capability.
    pub fn provides(mut self, capability: impl Into<String>) -> Self {
        self.meta.provides.push(capability.into());
        self
    }
    /// Declare a required capability.
    pub fn requires(mut self, capability: impl Into<String>) -> Self {
        self.meta.requires.push(capability.into());
        self
    }
    /// Declare an observed capability.
    pub fn observes(mut self, capability: impl Into<String>) -> Self {
        self.meta.observes.push(capability.into());
        self
    }
    /// Add a search tag.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.meta.tags.push(tag.into());
        self
    }
    /// Add a search alias.
    pub fn alias(mut self, alias: impl Into<String>) -> Self {
        self.meta.aliases.push(alias.into());
        self
    }
    /// Set the catalog category (M3.4) — canonicalized (`UI` → `std:UI`).
    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.meta.category = Some(crate::caps::canonical(&category.into()));
        self
    }
    /// Add/override a UI hint for a field.
    pub fn ui_hint(mut self, field: impl Into<String>, hint: impl Into<String>) -> Self {
        self.meta.ui_hints.insert(field.into(), hint.into());
        self
    }

    /// Finish — fields are sorted into canonical order for stable serialization.
    pub fn build(mut self) -> ComponentMeta {
        self.meta.fields.sort_by(|a, b| a.name.cmp(&b.name));
        self.meta
    }
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn parse_schema(
    schema: &Value,
) -> Result<(Vec<FieldSpec>, BTreeMap<String, String>), RegistryError> {
    let obj = schema.as_object().ok_or(RegistryError::RootNotObject)?;
    if obj.get("type").and_then(Value::as_str) != Some("object") {
        return Err(RegistryError::RootNotObject);
    }
    let props = obj
        .get("properties")
        .ok_or(RegistryError::MissingProperties)?;
    let props = props
        .as_object()
        .ok_or_else(|| RegistryError::PropertiesNotObject(json_type_name(props)))?;

    // required[] first so its entries can be validated against properties in declaration order.
    let mut required: HashSet<&str> = HashSet::new();
    if let Some(req) = obj.get("required") {
        let arr = req
            .as_array()
            .ok_or(RegistryError::RequiredNotStringArray)?;
        for v in arr {
            let name = v.as_str().ok_or(RegistryError::RequiredNotStringArray)?;
            if !props.contains_key(name) {
                return Err(RegistryError::RequiredUnknownField(name.to_string()));
            }
            required.insert(name);
        }
    }

    let mut fields = Vec::with_capacity(props.len());
    let mut hints = BTreeMap::new();
    for (fname, fschema) in props {
        let fobj = fschema
            .as_object()
            .ok_or_else(|| RegistryError::FieldNotObject {
                field: fname.clone(),
            })?;
        let tyv = fobj
            .get("type")
            .ok_or_else(|| RegistryError::FieldMissingType {
                field: fname.clone(),
            })?;
        let tys = tyv
            .as_str()
            .ok_or_else(|| RegistryError::FieldTypeNotString {
                field: fname.clone(),
            })?;
        let ty =
            FieldType::from_schema(tys).ok_or_else(|| RegistryError::FieldUnsupportedType {
                field: fname.clone(),
                ty: tys.to_string(),
            })?;
        let format = fobj.get("format").and_then(Value::as_str).map(String::from);
        if let Some(desc) = fobj.get("description").and_then(Value::as_str) {
            hints.insert(fname.clone(), desc.to_string());
        }
        fields.push(FieldSpec {
            name: fname.clone(),
            ty,
            required: required.contains(fname.as_str()),
            format,
        });
    }
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((fields, hints))
}

/// Registration / schema-validation errors — every variant carries an actionable message.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// Empty component name.
    #[error("component name must not be empty")]
    EmptyName,
    /// A kind with this name is already registered.
    #[error("component kind {0:?} is already registered")]
    Duplicate(String),
    /// Schema root is not `{{\"type\": \"object\", …}}`.
    #[error("schema root must be an object with \"type\": \"object\"")]
    RootNotObject,
    /// No `properties` map.
    #[error("schema is missing \"properties\" (an object of field-name → field-schema)")]
    MissingProperties,
    /// `properties` is not an object.
    #[error("\"properties\" must be an object, got {0}")]
    PropertiesNotObject(&'static str),
    /// A field's schema is not an object.
    #[error("field {field:?}: its schema must be an object")]
    FieldNotObject {
        /// Offending field.
        field: String,
    },
    /// A field has no `type`.
    #[error("field {field:?}: missing \"type\"")]
    FieldMissingType {
        /// Offending field.
        field: String,
    },
    /// A field's `type` is not a string.
    #[error("field {field:?}: \"type\" must be a string")]
    FieldTypeNotString {
        /// Offending field.
        field: String,
    },
    /// A field's `type` is not a supported scalar.
    #[error("field {field:?}: unsupported type {ty:?} (use integer, number, boolean, or string)")]
    FieldUnsupportedType {
        /// Offending field.
        field: String,
        /// The unsupported type string.
        ty: String,
    },
    /// `required` is not an array of strings.
    #[error("\"required\" must be an array of field-name strings")]
    RequiredNotStringArray,
    /// `required` names a field absent from `properties`.
    #[error("\"required\" lists {0:?}, which is not in \"properties\"")]
    RequiredUnknownField(String),
    /// The provided text is not valid JSON.
    #[error("schema is not valid JSON: {0}")]
    InvalidJson(String),
}

struct Registered {
    entity: Entity,
    meta: ComponentMeta,
}

/// The component metadata registry over a [`World`] backend. Owns the world it registers into for
/// M1.3 (the commit pipeline will reorganize ownership in M1–2); registration feeds capabilities in
/// as relationship pairs, and capability queries go back out through the wrapper.
pub struct Registry<W: World> {
    world: W,
    provides_rel: Entity,
    requires_rel: Entity,
    observes_rel: Entity,
    capabilities: HashMap<String, Entity>,
    kinds: HashMap<String, Registered>,
    by_entity: HashMap<Entity, String>,
    // M12.1 (ADR-045) — the Rules-layer vocabulary: the events a rule can trigger on (When) and the action
    // verbs it can run (Then). `BTreeMap` so enumeration is sorted + deterministic (stable builder dropdowns
    // + reproducible tests). Components carry their own field schema (above); events/actions are name-keyed.
    events: BTreeMap<String, EventMeta>,
    actions: BTreeMap<String, ActionMeta>,
    // M12.3 (ADR-047) — the WASM-plugin vocabulary (the honest ceiling): the algorithmic components a
    // `RunPlugin` rule action may invoke. Name-keyed, sorted/deterministic like events/actions.
    plugins: BTreeMap<String, PluginMeta>,
}

impl<W: World> Registry<W> {
    /// Create a registry over `world`, interning the three capability relationships.
    pub fn new(mut world: W) -> Self {
        let provides_rel = world.create_entity();
        let requires_rel = world.create_entity();
        let observes_rel = world.create_entity();
        Self {
            world,
            provides_rel,
            requires_rel,
            observes_rel,
            capabilities: HashMap::new(),
            kinds: HashMap::new(),
            by_entity: HashMap::new(),
            events: BTreeMap::new(),
            actions: BTreeMap::new(),
            plugins: BTreeMap::new(),
        }
    }

    /// Register a component kind. Validates name/uniqueness, interns its capabilities, and records
    /// the kind as an entity carrying the corresponding pairs.
    ///
    /// # Errors
    /// [`RegistryError::EmptyName`] or [`RegistryError::Duplicate`].
    pub fn register(&mut self, meta: ComponentMeta) -> Result<Entity, RegistryError> {
        if meta.name.is_empty() {
            return Err(RegistryError::EmptyName);
        }
        if self.kinds.contains_key(&meta.name) {
            return Err(RegistryError::Duplicate(meta.name.clone()));
        }
        let kind = self.world.create_entity();
        for cap in &meta.provides {
            let c = self.capability(cap);
            self.world.add_pair(kind, self.provides_rel, c);
        }
        for cap in &meta.requires {
            let c = self.capability(cap);
            self.world.add_pair(kind, self.requires_rel, c);
        }
        for cap in &meta.observes {
            let c = self.capability(cap);
            self.world.add_pair(kind, self.observes_rel, c);
        }
        self.by_entity.insert(kind, meta.name.clone());
        self.kinds
            .insert(meta.name.clone(), Registered { entity: kind, meta });
        Ok(kind)
    }

    /// Intern a capability name to a stable [`Entity`].
    fn capability(&mut self, name: &str) -> Entity {
        if let Some(&e) = self.capabilities.get(name) {
            return e;
        }
        let e = self.world.create_entity();
        self.capabilities.insert(name.to_string(), e);
        e
    }

    /// Kind names that **provide** `capability`, answered through the wrapper's pair-match query.
    pub fn providers_of(&self, capability: &str) -> Vec<String> {
        self.query_capability(self.provides_rel, capability)
    }
    /// Kind names that **require** `capability` (the candidates that bind to a provider).
    pub fn requirers_of(&self, capability: &str) -> Vec<String> {
        self.query_capability(self.requires_rel, capability)
    }
    /// Kind names that **observe** `capability`.
    pub fn observers_of(&self, capability: &str) -> Vec<String> {
        self.query_capability(self.observes_rel, capability)
    }

    fn query_capability(&self, rel: Entity, capability: &str) -> Vec<String> {
        let Some(&cap) = self.capabilities.get(capability) else {
            return Vec::new();
        };
        let query = self.world.build_query(&[Clause::with(Term::Pair {
            rel,
            target: Target::Exact(cap),
        })]);
        let mut names = Vec::new();
        self.world.for_each_match(&query, &mut |e| {
            if let Some(name) = self.by_entity.get(&e) {
                names.push(name.clone());
            }
        });
        names.sort();
        names
    }

    /// Metadata for a registered kind.
    pub fn meta(&self, kind: &str) -> Option<&ComponentMeta> {
        self.kinds.get(kind).map(|r| &r.meta)
    }
    /// The kind entity for a registered kind (its handle in the world).
    pub fn entity(&self, kind: &str) -> Option<Entity> {
        self.kinds.get(kind).map(|r| r.entity)
    }
    /// Number of registered kinds.
    pub fn len(&self) -> usize {
        self.kinds.len()
    }
    /// Whether no kinds are registered.
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }
    /// All registered metadata (unordered).
    pub fn metas(&self) -> impl Iterator<Item = &ComponentMeta> {
        self.kinds.values().map(|r| &r.meta)
    }

    // ── M12.1 Rules vocabulary (ADR-045) — the "When" events + "Then" actions ──

    /// Register a rule **event** (the "When" vocabulary). Re-registering a name overwrites it.
    pub fn register_event(&mut self, meta: EventMeta) {
        self.events.insert(meta.name.clone(), meta);
    }

    /// Register a rule **action** verb (the closed "Then" vocabulary — the honest ceiling).
    pub fn register_action(&mut self, meta: ActionMeta) {
        self.actions.insert(meta.name.clone(), meta);
    }

    /// Whether `name` is a registered rule event — the typo-proof gate for a rule's `When`.
    #[must_use]
    pub fn has_event(&self, name: &str) -> bool {
        self.events.contains_key(name)
    }

    /// Whether `name` is a registered rule action verb — the typo-proof gate for a rule's `Then`.
    #[must_use]
    pub fn has_action(&self, name: &str) -> bool {
        self.actions.contains_key(name)
    }

    /// All registered events, sorted by name (the builder's "When" dropdown source).
    pub fn events(&self) -> impl Iterator<Item = &EventMeta> {
        self.events.values()
    }

    /// All registered action verbs, sorted by name (the builder's "Then" dropdown source).
    pub fn actions(&self) -> impl Iterator<Item = &ActionMeta> {
        self.actions.values()
    }

    // ── M12.3 plugin vocabulary (ADR-047) — the honest ceiling ──────────

    /// Register a WASM-plugin component (the algorithmic escape a `RunPlugin` action may invoke).
    /// Re-registering a name overwrites it.
    pub fn register_plugin(&mut self, meta: PluginMeta) {
        self.plugins.insert(meta.name.clone(), meta);
    }

    /// Whether `name` is a registered plugin — the typo-proof gate for a `RunPlugin` action's target.
    #[must_use]
    pub fn has_plugin(&self, name: &str) -> bool {
        self.plugins.contains_key(name)
    }

    /// Metadata for a registered plugin (its description + the determinism flag).
    #[must_use]
    pub fn plugin(&self, name: &str) -> Option<&PluginMeta> {
        self.plugins.get(name)
    }

    /// All registered plugins, sorted by name (the builder's plugin-picker source).
    pub fn plugins(&self) -> impl Iterator<Item = &PluginMeta> {
        self.plugins.values()
    }
}
