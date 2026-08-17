"""`audit.py --self-test` — every case is a drift that a gate like this exists to catch, and every
case is paired with a control in which that same drift is repaired.

A self-test that only asserts "the drifted tree produces a finding" can pass for the wrong reason:
some *other* guard fires on the same input, the case goes green, and the guard it was written for
can be deleted without anyone noticing. That happened while this file's predecessor was being
written — two of six GPU cases passed under their own mutation, each caught by a different check.

So each case here runs **twice**: once drifted (the named finding must appear) and once repaired
(the named finding must be gone, and nothing else may appear either). The pair is the mutation test:
it fails the moment the guard stops discriminating, in CI, on every run — rather than the one time
somebody remembers to revert a fix by hand.

Cases 14 and 15 are not hypothetical drifts. They are two bugs *this tool shipped with* during the
session that wrote it: `-> [f64; 8]` read as "returns nothing" because a regex stopped at the
semicolon, and a `match` arm inside a `json!` reply unbalanced the bracket depth because `=>`
contains a `>`. Both produced confident wrong answers about real commands. They are pinned here so
the repair cannot be undone quietly.
"""

from __future__ import annotations

import os
import re
import shutil
import tempfile

import audit

# ── the fixture ───────────────────────────────────────────────────────────────────────────────────

BASE_MAIN = """\
use tauri::State;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub world_x: f64,
    pub label: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityInfo {
    pub id: String,
    pub parent_id: Option<String>,
    pub placement: Placement,
}

#[tauri::command]
fn entity_info(state: State<AppState>, id: String) -> EntityInfo {
    EntityInfo { id, parent_id: None, placement: Placement { world_x: 0.0, label: String::new() } }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            entity_info,
        ])
        .run(tauri::generate_context!())
        .unwrap();
}
"""

BASE_LIB = "pub struct Unused;\n"

BASE_PROTOCOL = """\
export interface Placement {
  worldX: number;
  label: string;
}

export interface EntityInfo {
  id: string;
  parentId: string | null;
  placement: Placement;
}
"""

BASE_SESSION = """\
export class TauriClient {
  entityInfo(id: string): Promise<EntityInfo> {
    return this.core.invoke<EntityInfo>("entity_info", { id });
  }
}
"""

FILES = {
    "editor-shell/src-tauri/src/main.rs": BASE_MAIN,
    "editor-shell/src/lib.rs": BASE_LIB,
    "editor/src/transport/protocol.ts": BASE_PROTOCOL,
    "editor/src/transport/session.ts": BASE_SESSION,
    "editor/src/store/project.ts": "export interface ProjectInfo { name: string; }\n",
    "editor/src/store/play.ts": "export interface PlayInfo { running: boolean; }\n",
    "core/src/lib.rs": "pub struct Nothing;\n",
}


def build(tmp: str, overrides: dict[str, str] | None = None, drop: set[str] | None = None) -> str:
    root = os.path.join(tmp, "repo")
    files = dict(FILES)
    files.update(overrides or {})
    for rel in drop or set():
        files.pop(rel, None)
    for rel, text in files.items():
        path = os.path.join(root, *rel.split("/"))
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text)
    return root


# ── the cases ─────────────────────────────────────────────────────────────────────────────────────
#
# (name, expect regex, drifted kwargs, repaired kwargs). The regex must match text only the intended
# guard produces — a bare check name would let a neighbouring guard satisfy the case.

CASES: list[tuple[str, str, dict, dict]] = [
    (
        "a command that never reaches generate_handler!",
        r"generate_handler.*never lists",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       BASE_MAIN.replace("            entity_info,\n", "")}},
        {},
    ),
    (
        "the UI invokes a name the shell does not register (the prompt-40 class)",
        r'invoke\("entity_info"\) names a command the shell does not register',
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       BASE_MAIN.replace("fn entity_info(", "fn entity_details(")
                                .replace("entity_info,", "entity_details,")}},
        {},
    ),
    (
        "an argument key the command does not accept",
        r"sends `entityId`, which the command does not accept",
        {"overrides": {"editor/src/transport/session.ts":
                       BASE_SESSION.replace("{ id }", "{ entityId: id }")}},
        {},
    ),
    (
        "a required argument the caller never sends",
        r"does not send `id`, which the command requires",
        {"overrides": {"editor/src/transport/session.ts":
                       BASE_SESSION.replace('"entity_info", { id }', '"entity_info", {}')}},
        {},
    ),
    (
        # The reply is fine; the type it CARRIES is not. Before the reader followed a reply past its
        # own keys, this passed silently — and it is not a hypothetical: moving six fields off a
        # `RuleSummary` and into the `RuleData` it carries (a strictly better DTO) moved all six out
        # of the audit's reach in the same commit. A gate whose coverage shrinks when the code
        # improves is a gate that argues for the worse code.
        "a renamed field ONE LEVEL DOWN, inside a type the reply carries",
        r"`EntityInfo\.placement` is read as `Placement`, which requires \['worldXPos'\]",
        {"overrides": {"editor/src/transport/protocol.ts":
                       BASE_PROTOCOL.replace("  worldX: number;", "  worldXPos: number;")}},
        {},
    ),
    (
        "a scalar KIND conflict one level down (Rust String read as a TS number)",
        r"`EntityInfo\.placement\.label` reads number where the reply is string",
        {"overrides": {"editor/src/transport/protocol.ts":
                       BASE_PROTOCOL.replace("  label: string;", "  label: number;")}},
        {},
    ),
    (
        "a nested field read as a list that the shell sends as one object",
        r"`EntityInfo\.placement` is a list in TypeScript, but `EntityInfo\.placement` is `Placement`",
        {"overrides": {"editor/src/transport/protocol.ts":
                       BASE_PROTOCOL.replace("  placement: Placement;", "  placement: Placement[];")}},
        {},
    ),
    (
        "a reply field the UI requires and the shell renamed (the C6 class)",
        r"requires \['parentId'\].*never sends",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       BASE_MAIN.replace('#[serde(rename_all = "camelCase")]\n', "")}},
        {},
    ),
    (
        "a list on one side and a single value on the other",
        r"expects a single value, but the command returns `Vec<EntityInfo>`",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       BASE_MAIN.replace("-> EntityInfo {", "-> Vec<EntityInfo> {")
                                .replace("EntityInfo { id, parent_id: None }",
                                         "vec![EntityInfo { id, parent_id: None }]")}},
        {},
    ),
    (
        "a caller reading a reply from a command that returns nothing",
        r"reads a reply, but the command returns nothing",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       BASE_MAIN.replace("-> EntityInfo {", "{")
                                .replace("EntityInfo { id, parent_id: None }", "let _ = id;")}},
        {},
    ),
    (
        "an audit root that does not exist",
        r"the audit root does not exist",
        {"root": "<missing>"},
        {},
    ),
    (
        "a required file that has moved",
        r"is on the audit's source list and is not in the tree",
        {"drop": {"editor-shell/src/lib.rs"}},
        {},
    ),
    (
        "a tree with no #[tauri::command] anywhere",
        r"no #\[tauri::command\] was found anywhere",
        {"overrides": {"editor-shell/src-tauri/src/main.rs": "fn main() {}\n",
                       "editor/src/transport/session.ts": "export const x = 1;\n"}},
        {},
    ),
    (
        "a shell with commands and no generate_handler!",
        r"no `generate_handler!\[\.\.\]` was found",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       BASE_MAIN[: BASE_MAIN.index("fn main()")] + "fn main() {}\n"}},
        {},
    ),
    (
        "a UI that no longer calls the shell anywhere",
        r"no `invoke\(\.\.\)` call site was found",
        {"overrides": {"editor/src/transport/session.ts": "export const nothing = 1;\n"}},
        {},
    ),
    (
        "a registered command the reader could not find (the parser's own blind spot)",
        r"is registered, but the audit found no `#\[tauri::command\] fn",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       BASE_MAIN.replace("            entity_info,\n",
                                         "            entity_info,\n            built_by_a_macro,\n")}},
        {},
    ),
    (
        "a command name computed at the call site",
        r"call site\(s\) pass the command name as a variable",
        {"overrides": {"editor/src/transport/session.ts":
                       BASE_SESSION.replace('"entity_info", { id }', "cmd, { id }")}},
        {},
    ),
    (
        "a match arm inside a json! reply, which must not abandon the reply (this tool's own bug)",
        r"requires \['mode'\].*json! literal it builds never sends",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       BASE_MAIN.replace(
                           "fn entity_info(state: State<AppState>, id: String) -> EntityInfo {\n"
                           "    EntityInfo { id, parent_id: None, placement: Placement { world_x: 0.0, label: String::new() } }\n}",
                           "fn entity_info(state: State<AppState>, id: String) -> serde_json::Value {\n"
                           "    serde_json::json!({\n"
                           '        "id": id,\n'
                           '        "kind": match id.len() { 0 => "empty", _ => "named" },\n'
                           '        "tags": v.iter().map(|t| t).collect::<Vec<_>>(),\n'
                           "    })\n}"),
                       "editor/src/transport/protocol.ts":
                       "export interface EntityInfo {\n  id: string;\n  kind: string;\n"
                       "  tags: string[];\n  mode: string;\n}\n"}},
        # Repaired: the shell also sends `mode`. If the reader ever gives up on the match arm again
        # it resolves nothing, reports nothing, and this control passes while the drifted case fails.
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       BASE_MAIN.replace(
                           "fn entity_info(state: State<AppState>, id: String) -> EntityInfo {\n"
                           "    EntityInfo { id, parent_id: None, placement: Placement { world_x: 0.0, label: String::new() } }\n}",
                           "fn entity_info(state: State<AppState>, id: String) -> serde_json::Value {\n"
                           "    serde_json::json!({\n"
                           '        "id": id,\n'
                           '        "kind": match id.len() { 0 => "empty", _ => "named" },\n'
                           '        "tags": v.iter().map(|t| t).collect::<Vec<_>>(),\n'
                           '        "mode": "live",\n'
                           "    })\n}"),
                       "editor/src/transport/protocol.ts":
                       "export interface EntityInfo {\n  id: string;\n  kind: string;\n"
                       "  tags: string[];\n  mode: string;\n}\n"}},
    ),
    (
        "an untyped reply the caller reads as a typed one",
        r"has no field names to compare",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       BASE_MAIN.replace("-> EntityInfo {", "-> serde_json::Value {")
                                .replace("EntityInfo { id, parent_id: None }",
                                         "build_it(id)")}},
        {},
    ),
]


# ── mutations of the READER itself ────────────────────────────────────────────────────────────────
#
# The two bugs above are not drifts in the tree; they are drifts in this tool. A tree fixture cannot
# pin them, because the *correct* tool produces no finding at all and "no finding" is what a missing
# guard produces too. So these cases put the buggy implementation back, require the wrong answer to
# reappear, then restore the fix and require it to be gone. That is a mutation test in the strict
# sense: the assertion is about the repair, and it fails if the repair is reverted.


def _old_return_type(src: str, i: int) -> str:
    """The original: a regex stopping at `{` OR `;`. `-> [f64; 8]` contains a semicolon."""
    m = re.match(r"\s*->\s*([^{;]+)\{", src[i:])
    return " ".join(m.group(1).split()) if m else "()"


def _old_angle(src: str, i: int) -> bool:
    """The original: every `<`/`>` counted as a bracket, including the `>` in a match arm's `=>`."""
    return True


READER_MUTATIONS: list[tuple[str, str, str, object, dict]] = [
    (
        "a fixed-size array must not read as no return value at all",
        r"reads a reply, but the command returns nothing",
        "rustipc._return_type",
        _old_return_type,
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       BASE_MAIN.replace("-> EntityInfo {", "-> [f64; 8] {")
                                .replace("EntityInfo { id, parent_id: None, placement: Placement { world_x: 0.0, label: String::new() } }", "[0.0; 8]"),
                       "editor/src/transport/session.ts":
                       BASE_SESSION.replace("invoke<EntityInfo>", "invoke<number[]>")}},
    ),
    (
        "a `=>` in a reply literal must not make the whole reply unreadable",
        r"has no field names to compare",
        "rustipc._angle_is_bracket",
        _old_angle,
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       BASE_MAIN.replace(
                           "fn entity_info(state: State<AppState>, id: String) -> EntityInfo {\n"
                           "    EntityInfo { id, parent_id: None, placement: Placement { world_x: 0.0, label: String::new() } }\n}",
                           "fn entity_info(state: State<AppState>, id: String) -> serde_json::Value {\n"
                           "    serde_json::json!({\n"
                           '        "id": id,\n'
                           '        "kind": match id.len() { 0 => "empty", _ => "named" },\n'
                           "    })\n}"),
                       "editor/src/transport/protocol.ts":
                       "export interface EntityInfo {\n  id: string;\n  kind: string;\n}\n"}},
    ),
]


def _findings(tmp: str, spec: dict) -> list[audit.Finding]:
    if spec.get("root") == "<missing>":
        return audit.run(os.path.join(tmp, "no-such-dir"))[0]
    root = build(tmp, spec.get("overrides"), spec.get("drop"))
    return audit.run(root)[0]


def run() -> int:
    ok = True
    for name, pattern, drifted, repaired in CASES:
        rx = re.compile(pattern, re.S)
        with tempfile.TemporaryDirectory() as tmp:
            got = _findings(tmp, drifted)
        hit = next((f for f in got if rx.search(f.message)), None)
        with tempfile.TemporaryDirectory() as tmp:
            clean = _findings(tmp, repaired)
        still = next((f for f in clean if rx.search(f.message)), None)

        if hit is None:
            print(f"FAIL  missed: {name}")
            for f in got:
                print(f"        got [{f.check}] {f.message[:130]}")
            ok = False
        elif still is not None:
            # The mutation test. A case that also fires on the repaired tree is not discriminating —
            # it would stay green after the guard it was written for is deleted.
            print(f"FAIL  fires on the REPAIRED tree too, so it proves nothing: {name}")
            print(f"        {still.message[:150]}")
            ok = False
        elif clean:
            print(f"FAIL  the repaired tree is not clean: {name}")
            for f in clean:
                print(f"        got [{f.check}] {f.message[:130]}")
            ok = False
        else:
            print(f"pass  caught, and only when drifted: {name}")
            print(f"        → {hit.message[:150]}")

    import rustipc

    for name, pattern, target, buggy, spec in READER_MUTATIONS:
        rx = re.compile(pattern, re.S)
        mod, attr = target.split(".")
        assert mod == "rustipc"
        good = getattr(rustipc, attr)
        setattr(rustipc, attr, buggy)
        try:
            with tempfile.TemporaryDirectory() as tmp:
                mutated = _findings(tmp, spec)
        finally:
            setattr(rustipc, attr, good)
        with tempfile.TemporaryDirectory() as tmp:
            fixed = _findings(tmp, spec)

        wrong = next((f for f in mutated if rx.search(f.message)), None)
        residue = next((f for f in fixed if rx.search(f.message)), None)
        if wrong is None:
            print(f"FAIL  the mutation produced no wrong answer, so the fix is unpinned: {name}")
            ok = False
        elif residue is not None:
            print(f"FAIL  the fix does not actually remove the wrong answer: {name}")
            ok = False
        elif fixed:
            print(f"FAIL  the repaired reader is not clean on this tree: {name}")
            for f in fixed:
                print(f"        got [{f.check}] {f.message[:130]}")
            ok = False
        else:
            print(f"pass  reverting the fix brings the wrong answer back: {name}")
            print(f"        → {wrong.message[:150]}")

    with tempfile.TemporaryDirectory() as tmp:
        base = audit.run(build(tmp))[0]
    if base:
        print("FAIL  the baseline fixture, which has no drift in it, is not clean:")
        for f in base:
            print(f"        [{f.check}] {f.message[:130]}")
        ok = False
    else:
        print("pass  a shell and a UI that agree stay clean")

    print()
    print(
        "self-test: every case is caught when drifted and absent when repaired — the pair is the "
        "mutation test, so a guard that stops discriminating fails here rather than going quiet."
        if ok
        else "self-test: FAILED — the gate does not catch what it claims to catch."
    )
    return 0 if ok else 1
