//! M12.4 (ADR-048) — the **Metrocalk MCP server** (rmcp 1.8.0, **stdio**). Exposes the validated compose
//! op-set as MCP **tools** so any MCP client (Claude/etc.) edits Rules/scenes through the **SAME**
//! `apply_composition` → commit pipeline as humans + plugins — never a raw mutation path. Launched with an
//! optional `.mtk` project path it edits in place: `metrocalk-mcp [project.mtk]`.
//!
//! Tools: **`composition_grammar`** (the SA-22 constrained-decoding JSON Schema the client constrains its
//! structured output by) · **`vocabulary`** (the registry components/fields) · **`apply_composition`**
//! (validate + apply a composition as one undoable tx, or reject-as-UX with an explained reason). The tool
//! LOGIC is in `metrocalk_mcp` (lib, tested); this is the thin rmcp wiring. AI is a **guest** — the engine
//! validates + applies (or refuses); the LLM only *proposes*.
//!
//! **stdio rule (MCP spec):** stdout is the JSON-RPC channel — this binary NEVER writes to stdout (no
//! `println!`); diagnostics, if any, go to stderr.

use std::path::PathBuf;

use rmcp::model::{CallToolResult, Content};
use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router, ServiceExt};
use serde::Deserialize;
use serde_json::json;

use metrocalk_core::compose::Composition;

/// The MCP server handler. Holds only the (Send + Sync) project path — the Flecs-backed engine is created +
/// dropped INSIDE each (synchronous) tool call (`metrocalk_mcp::apply_to_project`), never held in `self` or
/// across an await, so the handler stays `Send + Sync` with no `local` feature needed.
#[derive(Clone)]
struct Metrocalk {
    project: Option<PathBuf>,
}

/// The `apply_composition` tool input. `composition` is a free-form object validated internally against the
/// registry — the client should first call `composition_grammar` to constrain its structured output to the
/// real (SA-22) schema. Shape: `{ "ops": [ { "op": "setField"|"authorRule"|"authorStateMachine", ... } ] }`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ApplyRequest {
    /// The proposed composition of validated patches (see `composition_grammar` for the schema).
    composition: serde_json::Value,
}

/// A rejected/contained outcome as a successful tool call carrying `{ applied: false, error }` (the tool
/// SUCCEEDED; the composition was refused — every "no" explained, ADR-016).
fn rejected(reason: &str) -> CallToolResult {
    CallToolResult::success(vec![Content::text(
        json!({ "applied": false, "error": reason }).to_string(),
    )])
}

#[tool_router(server_handler)]
impl Metrocalk {
    // `&self` is required by rmcp's `#[tool]` macro contract; these two read tools don't need it.
    #[tool(
        description = "Return the SA-22 constrained-decoding JSON Schema for a composition. Constrain your structured output to this schema so every op is in-vocabulary (the allow-listed op set + real component names)."
    )]
    #[allow(clippy::unused_self)]
    fn composition_grammar(&self) -> String {
        serde_json::to_string(&metrocalk_mcp::grammar()).unwrap_or_default()
    }

    #[tool(
        description = "Return the registry vocabulary: the components and their fields' scalar types you may compose over."
    )]
    #[allow(clippy::unused_self)]
    fn vocabulary(&self) -> String {
        serde_json::to_string(&metrocalk_mcp::vocabulary()).unwrap_or_default()
    }

    #[tool(
        description = "Apply a validated composition (Rules / components / state machines) to the project as ONE undoable transaction through the engine's commit pipeline, or reject it with a plain-language reason (nothing applied). This is the SAME validated path a human or a plugin uses — never a raw mutation. Returns JSON { applied, error?, rules, stateMachines }."
    )]
    fn apply_composition(&self, Parameters(req): Parameters<ApplyRequest>) -> CallToolResult {
        // A malformed proposal is an explained rejection, not a crash.
        let composition: Composition = match serde_json::from_value(req.composition) {
            Ok(c) => c,
            Err(e) => return rejected(&format!("the composition isn't well-formed: {e}")),
        };
        let bytes = self.project.as_ref().and_then(|p| std::fs::read(p).ok());
        match metrocalk_mcp::apply_to_project(bytes.as_deref(), &composition) {
            Ok((new_bytes, res)) => {
                // Persist a SUCCESSFUL, applied composition back to the project file (if one is configured).
                if res.applied {
                    if let Some(p) = &self.project {
                        if let Err(e) = std::fs::write(p, &new_bytes) {
                            return rejected(&format!(
                                "applied, but could not save the project: {e}"
                            ));
                        }
                    }
                }
                let body = serde_json::to_string(&res).unwrap_or_default();
                CallToolResult::success(vec![Content::text(body)])
            }
            Err(e) => rejected(&e),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The project this server edits (optional): `metrocalk-mcp [project.mtk]`. Read tools work without one.
    let project = std::env::args().nth(1).map(PathBuf::from);
    let service = Metrocalk { project }
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
