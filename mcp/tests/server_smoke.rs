//! M12.4 (ADR-048) — a LIVE smoke test of the rmcp **stdio** server (the wiring `tools.rs` doesn't cover):
//! spawn the real `metrocalk-mcp` binary and speak newline-delimited JSON-RPC over its stdin/stdout — the
//! MCP `initialize` handshake, then `tools/list` (all three tools enumerated), then `tools/call` of the
//! `composition_grammar` read tool (the real SA-22 schema comes back). Headless + deterministic (no project,
//! no display, no network) — unlike the GUI .exe e2e, this runs in CI. Proves the server actually speaks the
//! protocol, not just that the tool logic compiles.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

/// Drive the server with a fixed JSON-RPC script, close stdin (EOF → the stdio server shuts down per the MCP
/// spec), and read everything it wrote back. Returns the concatenated stdout (all responses, newline-framed).
fn converse(requests: &[&str]) -> String {
    let exe = env!("CARGO_BIN_EXE_metrocalk-mcp");
    let mut child = Command::new(exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the metrocalk-mcp server");

    // Write every request, then drop stdin so the server sees EOF and exits cleanly (no orphaned child).
    {
        let mut stdin = child.stdin.take().expect("server stdin");
        for r in requests {
            writeln!(stdin, "{r}").expect("write request");
        }
        stdin.flush().expect("flush requests");
    }

    let mut out = String::new();
    child
        .stdout
        .take()
        .expect("server stdout")
        .read_to_string(&mut out)
        .expect("read server stdout");
    child.wait().expect("server exits on stdin EOF");
    out
}

#[test]
fn the_server_handshakes_lists_its_tools_and_returns_the_grammar() {
    // The MCP lifecycle over stdio: initialize → initialized → tools/list → tools/call (composition_grammar).
    let out = converse(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"composition_grammar","arguments":{}}}"#,
    ]);

    // The handshake completed — an `initialize` result carries `serverInfo`.
    assert!(out.contains("serverInfo"), "no initialize handshake: {out}");
    // `tools/list` enumerated ALL three tools (the client can discover the validated op-set).
    for tool in ["composition_grammar", "vocabulary", "apply_composition"] {
        assert!(out.contains(tool), "tools/list missing `{tool}`: {out}");
    }
    // `tools/call` of the read tool returned the real SA-22 grammar (its schema title).
    assert!(
        out.contains("MetrocalkComposition"),
        "composition_grammar didn't return the schema: {out}"
    );
}
