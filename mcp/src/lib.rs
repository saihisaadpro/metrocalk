//! M12.4 (ADR-048) — the **Metrocalk MCP server's tool LOGIC** (core-only, testable). The rmcp/stdio wiring
//! lives in `main.rs`; this is the validated **load → compose → save** the tools call, so any MCP client
//! (Claude/etc.) edits Rules/scenes through the **SAME `apply_composition` → commit pipeline** as humans and
//! plugins — never a raw mutation path. AI is a **guest**: the engine validates + applies (or refuses); the
//! LLM only *proposes* a `Composition` (constrained by the [`grammar`] this server hands it). Native-only
//! (the engine is Flecs-backed); `/core`/`/ecs` never link this crate (the AI-is-a-guest CI tripwire).

use metrocalk_core::compose::Composition;
use metrocalk_core::stdlib::{standard_actions, standard_components, standard_events};
use metrocalk_core::{apply_composition, composition_grammar, project, Engine, Registry};
use metrocalk_ecs::FlecsWorld;
use serde::Serialize;
use serde_json::{json, Value};

/// A recognizable peer id for compositions authored by the MCP server ("MCP" in hex). Only used for NEW
/// allocations (compose targets existing entities + AI-provided rule/machine ids), so it never collides with
/// a loaded project's ids.
const MCP_PEER: u64 = 0x004d_4350;

/// The registry vocabulary (components + events + actions) the compose path validates against.
fn registry() -> Registry<FlecsWorld> {
    let mut reg = Registry::new(FlecsWorld::new());
    for m in standard_components() {
        reg.register(m).expect("stdlib component registers");
    }
    for e in standard_events() {
        reg.register_event(e);
    }
    for a in standard_actions() {
        reg.register_action(a);
    }
    reg
}

/// **SA-22 grammar tool:** the constrained-decoding JSON Schema the MCP client constrains its structured
/// output by (so it can't even *propose* an out-of-schema op within the grammar). A pure read of the
/// registry vocabulary.
#[must_use]
pub fn grammar() -> Value {
    composition_grammar(&standard_components())
}

/// **Vocabulary read tool:** the registry's components + their fields' scalar types — what a client may
/// compose over (the typo-proof vocabulary, plain JSON).
#[must_use]
pub fn vocabulary() -> Value {
    let components: Vec<Value> = standard_components()
        .iter()
        .map(|c| {
            let fields: Vec<Value> = c
                .fields
                .iter()
                .map(|f| json!({ "name": f.name, "type": field_ty(f.ty) }))
                .collect();
            json!({ "name": c.name, "fields": fields })
        })
        .collect();
    json!({ "components": components })
}

fn field_ty(ty: metrocalk_core::FieldType) -> &'static str {
    use metrocalk_core::FieldType::{Boolean, Integer, Number, String as Str};
    match ty {
        Integer => "integer",
        Number => "number",
        Boolean => "boolean",
        Str => "string",
    }
}

/// The outcome of an `apply_composition` tool call — machine-readable + a human reason (ADR-016, the
/// every-"no" engine on AI). Serialized as the tool's structured result.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    /// True iff the composition validated + committed (one undoable tx).
    pub applied: bool,
    /// A plain-language rejection reason when `!applied` (every "no" explained); `None` on success.
    pub error: Option<String>,
    /// Rules in the project after the call.
    pub rules: usize,
    /// State machines in the project after the call.
    pub state_machines: usize,
}

/// **The headline write tool's logic:** load a `.mtk` project (or start fresh when `project_bytes` is
/// `None`), apply the AI's `composition` through the **validated commit pipeline** ([`apply_composition`] —
/// one undoable tx, all-or-nothing, rejected-as-UX), and return the **new project bytes** + a summary, or an
/// explained rejection (the project bytes returned **unchanged**, nothing applied). A client edits the same
/// pipeline as a human; the AI is never a raw path.
///
/// # Errors
/// A project that can't be parsed/loaded (a corrupt/too-new `.mtk`) — surfaced as a string, never a panic.
pub fn apply_to_project(
    project_bytes: Option<&[u8]>,
    composition: &Composition,
) -> Result<(Vec<u8>, ApplyResult), String> {
    let reg = registry();
    let mut engine = Engine::new(FlecsWorld::new(), MCP_PEER);
    if let Some(bytes) = project_bytes {
        let snapshot = project::parse(bytes).map_err(|e| e.to_string())?;
        engine.merge(&snapshot).map_err(|e| e.to_string())?;
    }
    match apply_composition(&mut engine, &reg, composition) {
        Ok(()) => {
            let bytes = project::build(&engine.snapshot());
            let res = ApplyResult {
                applied: true,
                error: None,
                rules: engine.rules().len(),
                state_machines: engine.state_machines().len(),
            };
            Ok((bytes, res))
        }
        Err(e) => {
            // Rejected-as-UX: nothing applied. Return the project UNCHANGED + the explained reason.
            let unchanged =
                project_bytes.map_or_else(|| project::build(&engine.snapshot()), <[u8]>::to_vec);
            let res = ApplyResult {
                applied: false,
                error: Some(e.to_string()),
                rules: engine.rules().len(),
                state_machines: engine.state_machines().len(),
            };
            Ok((unchanged, res))
        }
    }
}
