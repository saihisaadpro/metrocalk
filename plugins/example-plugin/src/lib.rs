//! M12.3 (ADR-047) — the example WASM plugin: `arrange`, a **deterministic procedural arrangement** (a
//! phyllotaxis / golden-angle spiral, rotated by a seed). This is **genuinely algorithmic** — a loop with
//! trig the no-code Rules layer can't express — yet **pure** (no host functions, no WASI, no random/clock),
//! so it's deterministic and eligible for the Play/replay lockstep path.
//!
//! Contract: input is JSON `{ ids: [entityKey…], seed: u64, spacing: f64, clientOpId?: string }`; output is
//! an **AiPatch** JSON (`{ clientOpId, ops: [{ op: "setField", id, component: "Transform", field, value }] }`)
//! — exactly the ADR-017 patch the host validates + commits as one undoable transaction. The plugin proposes;
//! the engine validates + applies. A plugin is never a raw mutation path.

use extism_pdk::*;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArrangeInput {
    /// The entity keys (the editor id-space) to arrange.
    ids: Vec<String>,
    /// Seed → a deterministic rotation of the whole pattern (same seed → byte-identical layout).
    #[serde(default)]
    seed: u64,
    /// Distance scale between successive points.
    #[serde(default = "default_spacing")]
    spacing: f64,
    /// The client op id echoed back in the AiPatch (optimistic-echo / rejection-as-UX).
    #[serde(default = "default_client_op")]
    client_op_id: String,
}

fn default_spacing() -> f64 {
    1.5
}
fn default_client_op() -> String {
    "plugin-arrange".to_string()
}

/// Arrange the given entities in a deterministic phyllotaxis spiral on the XZ plane (y = 0). Returns an
/// AiPatch the host validates + commits.
#[plugin_fn]
pub fn arrange(input: String) -> FnResult<String> {
    let req: ArrangeInput = serde_json::from_str(&input)
        .map_err(|e| WithReturnCode::new(Error::msg(format!("bad arrange input: {e}")), 1))?;

    // The golden angle (~137.5°) → the classic sunflower-seed packing; the seed rotates the whole pattern.
    const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;
    let seed_rotation = (req.seed as f64) * 0.618_033_988_749_895; // deterministic per-seed offset

    let mut ops: Vec<Value> = Vec::with_capacity(req.ids.len() * 3);
    for (i, id) in req.ids.iter().enumerate() {
        let fi = i as f64;
        let angle = fi * GOLDEN_ANGLE + seed_rotation;
        let radius = req.spacing * fi.sqrt();
        let x = radius * angle.cos();
        let z = radius * angle.sin();
        for (field, value) in [("px", x), ("py", 0.0_f64), ("pz", z)] {
            ops.push(json!({
                "op": "setField",
                "id": id,
                "component": "Transform",
                "field": field,
                "value": value,
            }));
        }
    }

    let patch = json!({ "clientOpId": req.client_op_id, "ops": ops });
    Ok(serde_json::to_string(&patch)?)
}
