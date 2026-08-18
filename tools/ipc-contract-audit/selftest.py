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

#: The one command several cases rewrite. Named rather than restated, because a case that swaps a
#: multi-line function by literal text is silently disarmed the moment the fixture is edited — the
#: `.replace()` becomes a no-op and the case goes on "passing" against an undrifted tree. That is a
#: vacuous pass in the self-test itself, and it happened while the reads fixture was being added.
#: `swap()` below makes it impossible: a replacement that matches nothing raises.
ENTITY_INFO_FN = """\
#[tauri::command]
fn entity_info(state: State<AppState>, id: String) -> EntityInfo {
    EntityInfo {
        id,
        parent_id: None,
        placement: Placement { world_x: 0.0, label: String::new(), source_file: String::new(), nickname: None, alias_name: None, frozen_name: None },
        seen_at: 0,
        tag_count: 0,
        facing: Facing::FaceNorth,
    }
}"""


#: The same command, rebuilt as a `json!` literal — the ad-hoc DTO shape, with a `match` arm whose
#: `=>` once unbalanced this reader's bracket depth. Two cases swap it in.
JSON_ENTITY_INFO_FN = """\
#[tauri::command]
fn entity_info(state: State<AppState>, id: String) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "kind": match id.len() { 0 => "empty", _ => "named" },
        "tags": v.iter().map(|t| t).collect::<Vec<_>>(),
    })
}"""


def swap(src: str, old: str, new: str) -> str:
    """`str.replace`, except that replacing nothing is an error rather than a quiet no-op."""
    if old not in src:
        raise AssertionError(
            f"the self-test fixture no longer contains the text a case drifts:\n  {old[:90]!r}\n"
            "The case would have run against an UNDRIFTED tree and proved nothing."
        )
    return src.replace(old, new)


BASE_MAIN = """\
use tauri::State;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub world_x: f64,
    pub label: String,
    pub source_file: String,
    // The `skip_serializing_if` half of the nullability contract, and a NEGATIVE pin on every run:
    // this key is OMITTED when `None`, never null, so `nickname?: string` is the honest reading and
    // must produce nothing. The `nullable` cases below flip only the ATTRIBUTE and only the TS
    // spelling — the Rust TYPE never changes — because the whole point of the check is that
    // `Option<T>` alone does not determine the wire form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    pub alias_name: Option<String>,
    pub frozen_name: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityInfo {
    pub id: String,
    pub parent_id: Option<String>,
    pub placement: Placement,
    pub seen_at: u64,
    pub tag_count: usize,
    pub facing: Facing,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Extra {
    pub kind: String,
}

// A serde enum on a reply field: a bare STRING on the wire, and a contract exactly as binding as a
// field name. `FaceUp2d` carries a per-variant rename whose value differs from the mechanical
// snake_case (`face_up2d` — a digit is not an uppercase letter, so serde does not break there),
// which is the real `Blend1d` -> `blend_1d` shape from this repository. A reader that skipped the
// attribute would report a drift that is not there.
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Facing {
    FaceNorth,
    FaceSouth,
    #[serde(rename = "face_up_2d")]
    FaceUp2d,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tagged {
    pub tagged_id: String,
    #[serde(flatten)]
    pub extra: Extra,
}

ENTITY_INFO_FN

// The same reply, reached by a command NO case rewrites. The E2E fixture reads its single-object
// fields off this one so that the two cases which turn `entity_info` into a `json!` literal do not
// also invalidate reads that are not what those cases are about.
#[tauri::command]
fn entity_seen(state: State<AppState>) -> EntityInfo {
    EntityInfo {
        id: String::new(),
        parent_id: None,
        placement: Placement { world_x: 0.0, label: String::new(), source_file: String::new(), nickname: None, alias_name: None, frozen_name: None },
        seen_at: 0,
        tag_count: 0,
        facing: Facing::FaceNorth,
    }
}

#[tauri::command]
fn entity_list(state: State<AppState>) -> Vec<EntityInfo> {
    Vec::new()
}

#[tauri::command]
fn entity_tagged(state: State<AppState>) -> Tagged {
    Tagged { tagged_id: String::new(), extra: Extra { kind: String::new() } }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            entity_info,
            entity_seen,
            entity_list,
            entity_tagged,
        ])
        .run(tauri::generate_context!())
        .unwrap();
}
""".replace("ENTITY_INFO_FN", ENTITY_INFO_FN)

#: The untyped half of the fixture — a spec that reads a reply without ever declaring its shape,
#: which is what 142 real files in `editor-shell/e2e` do. It is part of the BASE tree, not only of
#: the cases that drift it, so the read walk is exercised on every single run rather than only when
#: a reads case happens to run. Three of its lines are NEGATIVE pins that must never produce a
#: finding, and the "the repaired tree is not clean" assertion enforces them on every case:
#:
#:   * `expect(await invoke(..)).toBeTruthy()` — a call's paren, not a grouping paren. Reading the
#:     two as the same thing reported `EntityInfo` as failing to send `toBeTruthy`, which is how
#:     this guard came to exist.
#:   * `t.kind` on a struct carrying `#[serde(flatten)]` — flatten splices in keys no reader here
#:     can enumerate, so absence is unprovable, not false.
#:   * `items.length` / `.join` — JavaScript, not the reply.
BASE_E2E = """\
import { invoke } from "../pages/scaffold.js";

describe("what a spec reads without declaring it", () => {
  it("reads fields off a single reply", async () => {
    const info = await invoke("entity_seen");
    if (info.id !== "1") throw new Error("id");
    if (info.placement.label !== "") throw new Error("label");
    if (info.seenAt !== 0) throw new Error("seenAt");
    expect(await invoke("entity_seen")).toBeTruthy();
  });

  it("reads fields off a list reply, by index and through a callback", async () => {
    const items = await invoke("entity_list");
    if (items.length === 0) throw new Error("empty");
    if (items[0].placement.sourceFile !== "") throw new Error("sourceFile");
    if (items.some((e) => e.tagCount > 0)) throw new Error("tagCount");
    if (items.map((e) => e.id).join(",") === "") throw new Error("ids");
    for (const e of items) {
      if (!e.parentId) throw new Error("parentId");
    }
  });

  it("reads a key a flattened struct splices in, which cannot be called absent", async () => {
    const t = await invoke("entity_tagged");
    if (t.kind !== "thing") throw new Error("kind");
  });
});
"""

BASE_LIB = "pub struct Unused;\n"

BASE_PROTOCOL = """\
// Two more NEGATIVE pins, correct on every run: an `Option` read through a type ALIAS that resolves
// to a nullable, and one wrapped in `Readonly`. Both are honest declarations, and the first version
// of the `nullable` check reported both as drifts — it called `unwrap` without the alias table (the
// one caller in the file that did not) and `unwrap` peels `ReadonlyArray<T>` but not `Readonly<T>`.
// A gate that fires on correct code gets waived, and a waived gate checks nothing.
export type MaybeName = string | null;

export interface Placement {
  worldX: number;
  label: string;
  nickname?: string;
  aliasName: MaybeName;
  frozenName: Readonly<string | null>;
}

export type Facing = "face_north" | "face_south" | "face_up_2d";

export interface EntityInfo {
  id: string;
  parentId: string | null;
  placement: Placement;
  facing: Facing;
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
    "editor-shell/e2e/specs-entity/entity.e2e.js": BASE_E2E,
    "editor/src/transport/protocol.ts": BASE_PROTOCOL,
    "editor/src/transport/session.ts": BASE_SESSION,
    "editor/src/store/project.ts": "export interface ProjectInfo { name: string; }\n",
    "editor/src/store/play.ts": "export interface PlayInfo { running: boolean; }\n",
    # A struct named `Facing` in ANOTHER crate, deliberately. This is the real `Action` collision —
    # a struct in `core/src/rules.rs` and an unrelated enum in `editor-shell/src/actions.rs`, both
    # legitimate, both unambiguous in Rust, and indistinguishable to a reader that keys by bare name.
    # Every `variants` case below therefore also proves same-file resolution: if the reader picked
    # this struct for `EntityInfo.facing`, no variant comparison would happen at all and the cases
    # would fail as "missed".
    "core/src/lib.rs": (
        "pub struct Nothing;\n\n"
        "#[derive(serde::Serialize)]\n"
        "pub struct Facing {\n    pub degrees: f64,\n}\n"
    ),
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
        # The quietest drift on this whole boundary. A renamed FIELD reads `undefined` and something
        # renders blank; a renamed VARIANT arrives as a perfectly ordinary, non-null, correctly-typed
        # string that simply never equals what the UI compares it to. Nothing throws, nothing is
        # undefined, and every branch keyed on it is false forever. Changing `rename_all` is the
        # one-character version: `snake_case` -> `camelCase` and every wire name moves at once.
        "a `rename_all` that moves every variant's wire name (the quietest drift here)",
        r"`EntityInfo\.facing` is read as \['face_north'.*\], but `Facing` also sends "
        r"\['faceNorth', 'faceSouth'\]",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       swap(BASE_MAIN,
                            '#[serde(rename_all = "snake_case")]\npub enum Facing {',
                            '#[serde(rename_all = "camelCase")]\npub enum Facing {')}},
        {},
    ),
    (
        # The other direction, and the reason a rename SURVIVES review: the new name gets added on
        # both sides and the old one is left behind on one of them, where it reads as ordinary
        # defensive coding rather than as a branch that can never fire.
        "a union member the enum never sends — a comparison that is false forever",
        r"admits \['face_down'\], which `Facing` never sends",
        {"overrides": {"editor/src/transport/protocol.ts":
                       swap(BASE_PROTOCOL, '| "face_up_2d";', '| "face_up_2d" | "face_down";')}},
        {},
    ),
    (
        # A unit-only enum is a bare string. The moment ONE variant carries data, serde writes the
        # whole enum externally tagged — `{"FaceNorth": ..}` — and every string comparison the caller
        # makes is false against an object. Not reach loss: a disagreement.
        "an enum that stopped being a string on the wire, still read as one",
        r"is read as the string union .*but `Facing` is not a string on the wire: "
        r"`FaceSouth` carries data",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       swap(BASE_MAIN, "    FaceSouth,\n", "    FaceSouth(u32),\n")}},
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
        # The E2E spec must go too: it calls the same commands, so leaving it would keep the call-site
        # count non-zero and the guard would not fire — which is exactly what it did when the reads
        # fixture was added, and why this case failed until the drop was widened.
        {"overrides": {"editor/src/transport/session.ts": "export const nothing = 1;\n"},
         "drop": {"editor-shell/e2e/specs-entity/entity.e2e.js"}},
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
                       swap(BASE_MAIN, ENTITY_INFO_FN, JSON_ENTITY_INFO_FN),
                       "editor/src/transport/protocol.ts":
                       "export interface EntityInfo {\n  id: string;\n  kind: string;\n"
                       "  tags: string[];\n  mode: string;\n}\n"}},
        # Repaired: the shell also sends `mode`. If the reader ever gives up on the match arm again
        # it resolves nothing, reports nothing, and this control passes while the drifted case fails.
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       swap(BASE_MAIN, ENTITY_INFO_FN,
                            JSON_ENTITY_INFO_FN.replace('    })\n}', '        "mode": "live",\n    })\n}')),
                       "editor/src/transport/protocol.ts":
                       "export interface EntityInfo {\n  id: string;\n  kind: string;\n"
                       "  tags: string[];\n  mode: string;\n}\n"}},
    ),
    (
        "an untyped reply the caller reads as a typed one",
        r"has no field names to compare",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       swap(BASE_MAIN, ENTITY_INFO_FN,
                            "#[tauri::command]\nfn entity_info(state: State<AppState>, id: String) "
                            "-> serde_json::Value {\n    build_it(id)\n}")}},
        {},
    ),
    # ── the reads check: what untyped JavaScript asserts about a reply without declaring it ────────
    #
    # The drift in all three is on the RUST side and the spec is untouched — which is the shape of
    # the incident that motivated the check. `RuleSummary` collapsed to `{ id, rule }`, four specs
    # went on reading the six fields that had moved, and five green gates said nothing because none
    # of them opens this tree for anything but `invoke()` call sites.
    #
    # Each drifts a field the TypeScript interfaces do NOT declare, so only the reads guard can fire
    # and the case cannot be satisfied by `shape` or `nested` reporting the same break twice.
    (
        "a renamed reply field an untyped spec still reads (the D1 class)",
        r'invoke\("entity_seen"\) is read as `info`\.seenAt, but `EntityInfo` never sends `seenAt`',
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       swap(BASE_MAIN, "pub seen_at: u64,", "pub noticed_at: u64,")
                       .replace("seen_at: 0,", "noticed_at: 0,")}},
        {},
    ),
    (
        "a renamed field two levels down, reached by index through a list reply",
        r"is read as `items`\[0\]\.placement\.sourceFile, but `Placement` never sends `sourceFile`",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       swap(BASE_MAIN, "pub source_file: String,", "pub origin_file: String,")
                       .replace("source_file: String::new()", "origin_file: String::new()")}},
        {},
    ),
    (
        "a renamed field reached through a callback's element parameter",
        r"is read as `items`\[0\]\.tagCount, but `EntityInfo` never sends `tagCount`",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       swap(BASE_MAIN, "pub tag_count: usize,", "pub label_count: usize,")
                       .replace("tag_count: 0,", "label_count: 0,")}},
        {},
    ),
    # ── nullable ─────────────────────────────────────────────────────────────────────────────────
    #
    # Four cases over ONE pair of facts, because the pair is the whole point: `Option<T>` is two
    # different wire contracts and the TYPE does not say which. Cases 1–2 hold the Rust fixed and
    # drift the TS; case 3 holds the TS fixed and moves only the serde ATTRIBUTE, so a reader that
    # keyed on the type instead of the attribute passes 1–2 and fails 3. Case 4 is the asymmetry
    # control: a declaration that admits MORE than the shell can send must stay silent.
    (
        "a bare Option read as `?`, which under strictNullChecks admits undefined and not null",
        r"`EntityInfo\.parentId` is `Option<String>` with no `skip_serializing_if`.*"
        r"admits `undefined`, not `null`",
        {"overrides": {"editor/src/transport/protocol.ts":
                       swap(BASE_PROTOCOL, "parentId: string | null;", "parentId?: string;")}},
        {},
    ),
    (
        "a bare Option read as a plain required field, which admits neither",
        r"`EntityInfo\.parentId` is `Option<String>` with no `skip_serializing_if`.*admits neither",
        {"overrides": {"editor/src/transport/protocol.ts":
                       swap(BASE_PROTOCOL, "parentId: string | null;", "parentId: string;")}},
        {},
    ),
    (
        "an omitted key read as always present — the Rust TYPE is unchanged, only the attribute is",
        r"`EntityInfo\.placement\.nickname` is `Option<String>` with `skip_serializing_if`.*"
        r"`Placement\.nickname` is declared `string`, which says it is always there",
        {"overrides": {"editor/src/transport/protocol.ts":
                       swap(BASE_PROTOCOL, "nickname?: string;", "nickname: string;")}},
        {},
    ),
    (
        "the REPLY ITSELF is an Option, and the caller's type does not admit null",
        r'invoke<EntityInfo>\("entity_info"\).*the whole reply is `null` when it is `None`',
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       swap(BASE_MAIN, "fn entity_info(state: State<AppState>, id: String) -> EntityInfo {",
                            "fn entity_info(state: State<AppState>, id: String) -> Option<EntityInfo> {")}},
        {},
    ),
    (
        "the attribute alone flips the verdict: drop skip_serializing_if and `?` stops being honest",
        r"`EntityInfo\.placement\.nickname` is `Option<String>` with no `skip_serializing_if`.*"
        r"`Placement` declares `nickname\?: string`, which admits `undefined`, not `null`",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       swap(BASE_MAIN,
                            '    #[serde(skip_serializing_if = "Option::is_none")]\n'
                            "    pub nickname: Option<String>,",
                            "    pub nickname: Option<String>,")}},
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


def _field_case_for_variants(name: str, rename_all: str | None) -> str:
    """The one-table mistake: serde's FIELD rule applied to a variant.

    serde has two rename functions and they disagree on almost every input, because a field arrives
    snake_case and a variant arrives PascalCase. Under `rename_all = "snake_case"` the field rule is
    the identity — so `FaceNorth` would stay `FaceNorth` while the wire carries `face_north`, and
    every variant in the repository would be reported as drifted at once.
    """
    import rustipc as _r

    return _r.wire_case(name, rename_all)


def _ignore_variant_rename(attrs: str) -> None:
    """Skip `#[serde(rename = "..")]` on a variant — the FALSE-finding failure mode.

    This is the one that matters most for a check nobody will re-derive by hand: `Blend1d` is on the
    wire as `blend_1d` only because of this attribute (mechanical snake_case gives `blend1d`), so a
    reader that ignored it would confidently report a drift in code that is correct. A gate that
    cries wolf on correct code gets waived, and a waived gate checks nothing.
    """
    return None


#: Bound once, at import, so every mutation below calls the ORIGINAL even after the runner has
#: rebound the module attribute. A default argument was the first shape of this and it broke the
#: moment the function grew a parameter — the mutation silently received `aliases` as `_orig`.
_REAL_NULLABLE = audit._nullable_findings


def _null_from_type_alone(
    r_dto, t_ty, key, r_ft, t_ft, t_optional, r_skipped, cmd, where, path, aliases=None
):
    """`Option<T>` judged by the TYPE, ignoring `skip_serializing_if` — the plausible wrong reader.

    This is the version anyone would write first, and it is wrong in the direction that costs a gate
    its authority: it reports the honest `#[serde(skip_serializing_if)]` + `nickname?: string` pair
    as a drift. `Option<T>` does not determine the wire form — the attribute does — and a check that
    cries wolf on correct code gets waived, after which it checks nothing.

    Pinned as a reader mutation rather than a tree case because the CORRECT reader is silent here,
    and silence is also what a missing guard produces.
    """
    return _REAL_NULLABLE(
        r_dto, t_ty, key, r_ft, t_ft, t_optional, False, cmd, where, path, aliases
    )


def _null_without_aliases(
    r_dto, t_ty, key, r_ft, t_ft, t_optional, r_skipped, cmd, where, path, aliases=None
):
    """The alias table dropped — a `type MaybeName = string | null` field read as non-nullable.

    Every other resolver in this file passes `ts.aliases`; the first version of this one did not,
    and `tsipc.unwrap` resolves an alias only when handed the table. The result was a **blocking
    finding on correct code**, which is the failure mode that costs a gate its authority.
    """
    return _REAL_NULLABLE(
        r_dto, t_ty, key, r_ft, t_ft, t_optional, r_skipped, cmd, where, path, {}
    )


def _no_readonly_peel(t: str) -> str:
    """`_strip_readonly` as the identity — `Readonly<string | null>` stays wrapped.

    `unwrap` peels `ReadonlyArray<T>` because that changes the shape; `Readonly<T>` does not, so it
    never needed peeling until a reader started asking what is INSIDE. Same class as the alias bug —
    a false finding on correct code, not a missed one.
    """
    return t


def _old_return_type(src: str, i: int) -> str:
    """The original: a regex stopping at `{` OR `;`. `-> [f64; 8]` contains a semicolon."""
    m = re.match(r"\s*->\s*([^{;]+)\{", src[i:])
    return " ".join(m.group(1).split()) if m else "()"


def _old_angle(src: str, i: int) -> bool:
    """The original: every `<`/`>` counted as a bracket, including the `>` in a match arm's `=>`."""
    return True


READER_MUTATIONS: list[tuple[str, str, str, object, dict]] = [
    (
        "serde's variant rule is not its field rule, and one table for both is wrong",
        r"`Facing` also sends \['FaceNorth', 'FaceSouth'\]",
        "rustipc.variant_case",
        _field_case_for_variants,
        {},
    ),
    (
        "a per-variant #[serde(rename)] ignored invents a drift in correct code",
        r"`Facing` also sends \['face_up2d'\]",
        "rustipc.variant_rename",
        _ignore_variant_rename,
        {},
    ),
    (
        "a fixed-size array must not read as no return value at all",
        r"reads a reply, but the command returns nothing",
        "rustipc._return_type",
        _old_return_type,
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       swap(BASE_MAIN, ENTITY_INFO_FN,
                            "#[tauri::command]\nfn entity_info(state: State<AppState>, id: String) "
                            "-> [f64; 8] {\n    [0.0; 8]\n}"),
                       "editor/src/transport/session.ts":
                       BASE_SESSION.replace("invoke<EntityInfo>", "invoke<number[]>")}},
    ),
    (
        "a `=>` in a reply literal must not make the whole reply unreadable",
        r"has no field names to compare",
        "rustipc._angle_is_bracket",
        _old_angle,
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       swap(BASE_MAIN, ENTITY_INFO_FN,
                            JSON_ENTITY_INFO_FN.replace(
                                '        "tags": v.iter().map(|t| t).collect::<Vec<_>>(),\n', "")),
                       "editor/src/transport/protocol.ts":
                       "export interface EntityInfo {\n  id: string;\n  kind: string;\n}\n"}},
    ),
    (
        "an omitted key judged by its type alone invents a drift in correct code",
        r"`EntityInfo\.placement\.nickname`.*admits `undefined`, not `null`",
        "audit._nullable_findings",
        _null_from_type_alone,
        {},
    ),
    (
        "an alias that resolves to a nullable, read without the alias table, invents a drift",
        r"`EntityInfo\.placement\.aliasName`.*admits neither",
        "audit._nullable_findings",
        _null_without_aliases,
        {},
    ),
    (
        "a `Readonly<T | null>` left wrapped invents a drift",
        r"`EntityInfo\.placement\.frozenName`.*admits neither",
        "audit._strip_readonly",
        _no_readonly_peel,
        {},
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

    # A mutation names the module it patches, because the reader is no longer one file: the Rust
    # parser and the comparison layer can each hold a wrong answer, and both are worth pinning.
    modules = {"rustipc": rustipc, "audit": audit}

    for name, pattern, target, buggy, spec in READER_MUTATIONS:
        rx = re.compile(pattern, re.S)
        mod, attr = target.split(".")
        assert mod in modules, f"unknown mutation target module {mod!r}"
        good = getattr(modules[mod], attr)
        setattr(modules[mod], attr, buggy)
        try:
            with tempfile.TemporaryDirectory() as tmp:
                mutated = _findings(tmp, spec)
        finally:
            setattr(modules[mod], attr, good)
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


# This file is a MODULE — CI reaches it as `audit.py --self-test`. But it is also named `selftest.py`,
# it sits next to an executable `audit.py`, and the obvious thing to type is `python selftest.py`.
# Without this block that invocation defined 40-odd functions, called none of them, printed nothing
# and **exited 0** — a false green of exactly the shape `<test_and_ci_discipline>` §2 forbids, handed
# to whoever was most likely to be checking. It was believed once in this repository's own history:
# a session's baseline sweep ran it, read the silent 0 as a pass, and only the missing case list gave
# it away. A self-test that can be run wrong will be, so the wrong way is now the right way.
if __name__ == "__main__":
    raise SystemExit(run())
