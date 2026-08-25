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
import jsreads
import rustipc
import tsipc

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
        tags: Vec::new(),
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


#: A `json!` reply with structure BELOW its top level, in the two forms the macro permits: a nested
#: object literal, and a list built by `.map(|x| json!({..})).collect()` — the only way to get one,
#: since `json!` cannot parse a method chain on an array literal. Both are lifted from the real tree
#: (`colour_status.working` and `camera_probe.shotPlacements`), and until 2026-08-25 every key below
#: the first step was a name with nothing behind it: the walk stopped and counted itself unresolved.
PROBE_FN = """\
#[tauri::command]
fn entity_probe(state: State<AppState>) -> serde_json::Value {
    serde_json::json!({
        "id": "1",
        "where": {
            "label": "",
            "wired": true,
        },
        "marks": v.iter().map(|m| serde_json::json!({
            "markId": m.id,
            "asDirected": m.ok,
        })).collect::<Vec<_>>(),
    })
}"""

#: The spec that reads it — untyped, in plain `.js`, exactly as the 161 real E2E files do. The list
#: is read through an ALIAS with an empty default, because that is the form the real defect had and
#: it composes the two reach rules: the alias carries `marks` into the element, and the element's
#: shape now exists to compare against.
PROBE_E2E = """\
import { invoke } from "../pages/scaffold.js";

describe("a json! reply with structure below its top level", () => {
  it("reads through a sub-object and through a list element", async () => {
    const p = await invoke("entity_probe");
    if (p.where.label !== "") throw new Error("label");
    if (p.where.wired !== true) throw new Error("wired");
    const marks = p.marks ?? [];
    if (marks.filter((m) => !m.asDirected).length) throw new Error("asDirected");
  });
});
"""


#: THE ARGUMENT-SIDE FIXTURE. A payload struct that derives **only** `Deserialize` — which is what
#: seven of the real tree's argument types do (`AnimationGraphSaveRequest`, `TerrainEditArgs`,
#: `AssetLabProcessRequest`, …) and the reason `rustipc` keeps a second universe: a reader gated on
#: `Serialize` cannot see any of them, so before this existed `argfields` would have reported every
#: one as "no such struct" forever, in a sentence that reads like agreement.
#:
#: It lives in its own file because the case that matters most is a NAME COLLISION. `core/src/lib.rs`
#: declares a different `MarkRequest`, exactly as the real tree declares two `Composition`s — one in
#: `core/src/compose.rs` (`{ ops }`, the one `compose()` actually takes) and one in
#: `core/src/variant.rs` (`{ id, nodes }`, the one a bare-name lookup returns. A first draft of this
#: check resolved by bare name and produced three confident findings against the wrong struct on its
#: first run, every one of them about correct code.
#:
#: Two of its four fields are NEGATIVE PINS, enforced on every case by "the repaired tree is clean":
#:   * `pinned` carries `#[serde(default)]` and the TypeScript does not declare it — serde fills it
#:     in, so the caller is right to leave it out and this must produce NOTHING. `default` is the
#:     deserialize direction and `skip_serializing_if` is the serialize one; a reader that answered
#:     out of the wrong attribute would report this correct code as a drift on every run;
#:   * `note` is `Option<String>` and the TypeScript declares it optional — likewise silent.
MARKS_RS = """\
use serde::Deserialize;

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MarkRequest {
    pub label: String,
    pub at_frame: u32,
    #[serde(default)]
    pub pinned: bool,
    pub note: Option<String>,
}

/// FOUR SERDE READINGS, ALL OF THEM NEGATIVE PINS. Every field here is correct code that a reader
/// missing one attribute reports as drift, and the "repaired tree is clean" assertion enforces all
/// four on every single case in this file:
///
///   * `alias` is accepted on the way IN and never produced on the way out, so a caller sending
///     `kinds` is right — and a reader that missed it produced TWO findings at once, `kindList`
///     "never sent" and `kinds` "dropped in silence";
///   * `skip_deserializing` takes the field off the incoming payload entirely: the caller does not
///     send it, and it is not `Option` and carries no `default`, so nothing else here excuses it;
///   * `rename(deserialize = "q")` is the same contract in serde's second spelling, which the
///     `rename = ".."` pattern does not match;
///   * `kinds` reaches the TypeScript side through `interface MarkFilter extends MarkFilterBase`,
///     so a reader that drops the `extends` clause calls an inherited field undeclared.
#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct MarkFilter {
    #[serde(alias = "kinds")]
    pub kind_list: Vec<String>,
    #[serde(skip_deserializing)]
    pub resolved: bool,
    #[serde(rename(deserialize = "q"))]
    pub query: String,
}

/// The CONTAINER-ATTRIBUTE subject, and it is its own struct so that a container `default` does not
/// make every field of `MarkFilter` omissible and silence the four pins above — which is exactly
/// what happened when these attributes were first written there. `page` is not `Option`, carries no
/// field attribute, and the TypeScript does not declare it: it is correct code ONLY because the
/// container defaults, so a reader that does not read container attributes reports it as required.
/// `deny_unknown_fields` is here too, and it is the one attribute that changes what a finding SAYS
/// rather than whether there is one — see the case that pins its sentence.
#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct MarkPage {
    pub page: u32,
    pub size: u32,
}

/// The COLLISION subject, and it has its own struct so that it does not silence the five pins above.
/// `editor/src/store/play.ts` declares a different `MarkScope`, and `ts.types` is keyed by bare name
/// with the last file parsed winning — the same hazard `Dto.collides_with` records on the Rust side,
/// left unguarded on this side while the Rust side's docstring argued at length against exactly it.
/// A collision makes the name unusable, not merely ambiguous: `argfields` must REFUSE and count a
/// gap. Comparing against whichever declaration happened to be parsed last would report every field
/// of both, in a commit that touched neither.
#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct MarkScope {
    pub from_frame: u32,
    pub to_frame: u32,
}
"""

#: The decoy. Same bare name, different fields, another crate — and nothing in Rust is ambiguous
#: about it, because `marks::MarkRequest` says which one. Only a reader that throws the path away
#: has a problem, which is what the `_arg_struct` mutation below reintroduces.
DECOY_MARKS = """\
#[derive(serde::Serialize, serde::Deserialize)]
pub struct MarkRequest {
    pub decoy_only: String,
}
"""

#: The command that takes it, and the one line that makes the fixture a test of PATH resolution
#: rather than of name lookup: the parameter is `marks::MarkRequest`, not `MarkRequest`.
ENTITY_MARK_FN = """\
#[tauri::command]
fn entity_mark(state: State<AppState>, request: marks::MarkRequest) -> bool {
    true
}

#[tauri::command]
fn mark_filter(state: State<AppState>, filter: marks::MarkFilter) -> bool {
    true
}

#[tauri::command]
fn mark_scope(state: State<AppState>, scope: marks::MarkScope) -> bool {
    true
}

#[tauri::command]
fn mark_page(state: State<AppState>, paging: marks::MarkPage) -> bool {
    true
}"""


def with_probe(probe_fn: str = PROBE_FN) -> dict[str, str]:
    """The base tree plus `entity_probe` and the spec that reads it, for the nested-`json!` cases."""
    main = swap(
        BASE_MAIN,
        "#[tauri::command]\nfn entity_tagged(",
        probe_fn + "\n\n#[tauri::command]\nfn entity_tagged(",
    )
    main = swap(main, "            entity_tagged,\n", "            entity_tagged,\n            entity_probe,\n")
    return {
        "editor-shell/src-tauri/src/main.rs": main,
        "editor-shell/e2e/specs-probe/probe.e2e.js": PROBE_E2E,
    }


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

// A LIST OF OBJECTS inside a reply — the shape the alias cases need, and the shape the real defect
// had. Its own struct rather than a reuse of `Extra`, whose `kind` is the flatten pin: a case that
// drifted a field two pins share would not say which mechanism caught it.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub tag_id: String,
    pub as_directed: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityInfo {
    pub id: String,
    pub parent_id: Option<String>,
    pub placement: Placement,
    pub tags: Vec<Tag>,
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
        tags: Vec::new(),
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

ENTITY_MARK_FN

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            entity_info,
            entity_seen,
            entity_list,
            entity_tagged,
            entity_mark,
            mark_filter,
            mark_scope,
            mark_page,
        ])
        .run(tauri::generate_context!())
        .unwrap();
}
""".replace("ENTITY_INFO_FN", ENTITY_INFO_FN).replace("ENTITY_MARK_FN", ENTITY_MARK_FN)

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

  it("reads through a name given to a sub-path of the reply", async () => {
    const seen = await invoke("entity_seen");
    const where = seen.placement ?? {};
    if (where.worldX !== 0) throw new Error("worldX");

    const tags = seen.tags ?? [];
    if (tags.some((t) => t.asDirected)) throw new Error("asDirected");

    // NEGATIVE PINS, and the reason the alias rule has three conditions rather than one. Neither
    // name holds the reply's shape, and following either would INVENT a finding — the failure mode
    // that is worse than a missed one, because it gets the gate waived:
    //   * `first` is an ELEMENT and not the list — the chain ended in a builtin, so `path` is one
    //     step shorter than `steps` and following it would prefix the element's own fields with the
    //     list's path, which is a claim about a shape neither side has;
    //   * `fallback` defaults to another expression rather than to an empty literal, so what it
    //     holds when the key is absent is a shape this reader has never seen;
    //   * `shouted` is a string: the initializer does not end where the chain does.
    const first = seen.tags.at(0);
    const fallback = seen.placement ?? somethingElse;
    const shouted = where.label + "!";
    if (first.inventedElementKey || fallback.inventedKey || shouted === "") throw new Error("pins");
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

export interface MarkRequest {
  label: string;
  atFrame: number;
  note?: string | null;
}

export interface MarkFilterBase {
  kinds: string[];
}

/** `kinds` is INHERITED. A reader that throws the `extends` clause away calls it undeclared and
 *  reports a field the command requires as one the caller never sends — a blocking finding, in its
 *  most confident wording, about the most ordinary idiom in this file's language. */
export interface MarkFilter extends MarkFilterBase {
  q: string;
}

export interface MarkPage {
  size: number;
}

"""

BASE_SESSION = """\
export class TauriClient {
  entityInfo(id: string): Promise<EntityInfo> {
    return this.core.invoke<EntityInfo>("entity_info", { id });
  }

  // The SCOPE pin, and the reason `binding_type` asks which span CONTAINS the call rather than which
  // declaration is nearest. This method binds `request` to an entirely different type, in a body the
  // invoke below is not inside. A reader that took the first — or the nearest — annotation of that
  // name would compare `MarkRequest` against `EntityInfo` and report four fields that are not drifted.
  private remember(request: EntityInfo): void {
    void request;
  }

  entityMark(request: MarkRequest): Promise<boolean> {
    return this.core.invoke<boolean>("entity_mark", { request });
  }

  // NEGATIVE PIN: an interface member written WITHOUT a trailing semicolon. The scan that walks past
  // a return annotation looking for the body has to stop at `}` as well as at `;`, or it walks out
  // of this declaration, through the next method's parentheses, and adopts THAT body as the scope of
  // a parameter belonging to a type. Both bindings then span the same text and the tie decides it.
  // One semicolon here is the difference between silence and six blocking findings.
  private declare api: {
    markFilter(filter: MarkRequest): Promise<boolean>
  };

  markFilter(filter: MarkFilter): Promise<boolean> {
    return this.core.invoke<boolean>("mark_filter", { filter });
  }

  // NEGATIVE PIN for the SHORTHAND-ONLY rule. `{ request: mark }` is not a statement that the value
  // has `request`'s type — it is a statement that it does not. A reader that looked up the KEY's
  // name would find `request` right here, bound to something else entirely, and compare a payload
  // struct against a reply type. `{ k }` and `{ k: v }` look alike and mean different things.
  markScope(scope: MarkScope): Promise<boolean> {
    return this.core.invoke<boolean>("mark_scope", { scope });
  }

  markPage(paging: MarkPage): Promise<boolean> {
    return this.core.invoke<boolean>("mark_page", { paging });
  }

  entityMarkAlias(request: EntityInfo, mark: MarkRequest): Promise<boolean> {
    void request;
    return this.core.invoke<boolean>("entity_mark", { request: mark });
  }

  // NEGATIVE PIN: an UNANNOTATED binding shadows an annotated outer one. `outer` is declared at the
  // top of this method with a type; the inner block rebinds it with none. The inner declaration is
  // the one in effect and it says nothing, so the honest answer is "no type" — reading the outer
  // compares a payload struct against an unrelated reply type and reports every field of both.
  entityMarkTwice(): Promise<boolean> {
    const request: EntityInfo = this.cached;
    void request;
    {
      const request = this.buildMark();
      return this.core.invoke<boolean>("entity_mark", { request });
    }
  }
}
"""

FILES = {
    "editor-shell/src-tauri/src/main.rs": BASE_MAIN,
    "editor-shell/src/lib.rs": BASE_LIB,
    "editor-shell/e2e/specs-entity/entity.e2e.js": BASE_E2E,
    "editor/src/transport/protocol.ts": BASE_PROTOCOL,
    "editor/src/transport/session.ts": BASE_SESSION,
    "editor/src/store/project.ts": (
        "export interface ProjectInfo { name: string; }\n\n"
        "export interface MarkScope { fromFrame: number; toFrame: number; }\n"
    ),
    # A SECOND `MarkFilter`, in another swept file, with different fields. `ts.types` is keyed by
    # bare name and the last file parsed wins — the same hazard `Dto.collides_with` records on the
    # Rust side, and it was unguarded on this side while the Rust side's docstring argued at length
    # against exactly it. A collision makes the name unusable: `argfields` must refuse, not compare.
    "editor/src/store/play.ts": (
        "export interface PlayInfo { running: boolean; }\n\n"
        "export interface MarkScope { clip: string; loop: boolean; }\n"
    ),
    # BOTH halves of the collision live in files no case rewrites. Declared in `protocol.ts`, the
    # pin switched ITSELF off in every case that replaces that file wholesale: the second
    # declaration then stood alone, uncontested, and was compared against as though it were the
    # subject — a pin that is only armed while unrelated cases leave it alone.
    # A struct named `Facing` in ANOTHER crate, deliberately. This is the real `Action` collision —
    # a struct in `core/src/rules.rs` and an unrelated enum in `editor-shell/src/actions.rs`, both
    # legitimate, both unambiguous in Rust, and indistinguishable to a reader that keys by bare name.
    # Every `variants` case below therefore also proves same-file resolution: if the reader picked
    # this struct for `EntityInfo.facing`, no variant comparison would happen at all and the cases
    # would fail as "missed".
    "editor-shell/src/marks.rs": MARKS_RS,
    "core/src/lib.rs": (
        "pub struct Nothing;\n\n"
        "#[derive(serde::Serialize)]\n"
        "pub struct Facing {\n    pub degrees: f64,\n}\n\n"
    ) + DECOY_MARKS,
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
        "a struct with deny_unknown_fields REJECTS the payload, and the finding must say so",
        r"the whole payload is rejected — the struct carries",
        {"overrides": {"editor/src/transport/protocol.ts":
                       swap(BASE_PROTOCOL, "export interface MarkPage {\n  size: number;",
                            "export interface MarkPage {\n  cursor: string;\n  size: number;")}},
        {},
    ),
    # ── argfields: a payload compared field-by-field, not by the one key that carries it ──────────
    #
    # Every case below was INVISIBLE to this gate until 2026-08-25. Measured on HEAD's own auditor
    # first, byte-for-byte against a frozen copy of the tree: renaming a field inside `ClauseRequest`,
    # `PipeForgeOptions`, `EditTx` or `compose::Composition` left the verdict identical at 1 blocking,
    # and the one drift that WAS caught (`RuleData.event`) was caught by the reply-side walks, because
    # `RuleData` also comes back out of `list_rules` — not one `arguments` finding among the eight.
    (
        "a field the command REQUIRES that the caller's type does not declare at all",
        r"`request\.label` is required by `MarkRequest`.*does not declare it",
        {"overrides": {"editor/src/transport/protocol.ts":
                       swap(BASE_PROTOCOL, "  label: string;\n  atFrame: number;\n",
                            "  atFrame: number;\n")}},
        {},
    ),
    (
        "a field the command REQUIRES that the caller's type declares OPTIONAL",
        r"`request\.label` is required by `MarkRequest`.*is declared optional",
        {"overrides": {"editor/src/transport/protocol.ts":
                       swap(BASE_PROTOCOL, "  label: string;", "  label?: string;")}},
        {},
    ),
    (
        "a key the caller declares that the struct has no field for — dropped in silence",
        r"declares `request\.extra`.*has no such field.*DROPS it in silence",
        {"overrides": {"editor/src/transport/protocol.ts":
                       swap(BASE_PROTOCOL, "  atFrame: number;\n  note?",
                            "  atFrame: number;\n  extra: string;\n  note?")}},
        {},
    ),
    (
        "a field drifts inside a struct that derives ONLY Deserialize",
        r"`request\.atTick` is required by `MarkRequest`",
        {"overrides": {"editor-shell/src/marks.rs":
                       swap(MARKS_RS, "pub at_frame: u32,", "pub at_tick: u32,")}},
        {},
    ),
    (
        "a required field stops being defaulted, so the caller may no longer leave it out",
        r"`request\.pinned` is required by `MarkRequest`",
        {"overrides": {"editor-shell/src/marks.rs":
                       swap(MARKS_RS, "    #[serde(default)]\n    pub pinned: bool,",
                            "    pub pinned: bool,")}},
        {},
    ),
    (
        "the caller declares a LIST where the command takes one object",
        r"one is a list and the other is not",
        {"overrides": {"editor/src/transport/session.ts":
                       swap(BASE_SESSION, "entityMark(request: MarkRequest)",
                            "entityMark(request: MarkRequest[])")}},
        {},
    ),
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
        # A `json!` reply used to be enumerable exactly one level deep: the top-level keys became a
        # synthetic DTO with no field TYPES, so the walk could name `where` and could not go into
        # it. Every read below the first step was counted as "walk stopped early" — the same output
        # a reply nobody reads produces. Proven on the committed tree before this was built: renaming
        # a key inside `colour_status.working` left the gate at 0 blocking.
        "a renamed key INSIDE a json! reply's sub-object",
        r"`p`\.where\.wired, but `json!@entity_probe\.where` never sends `wired`",
        {"overrides": with_probe(PROBE_FN.replace('"wired": true,', '"wiredUp": true,'))},
        {"overrides": with_probe()},
    ),
    (
        # The same gap one shape over, and the one ADR-140 named on its way out. The E2E reads
        # `marks.filter((m) => !m.asDirected)`, so when the key goes missing `!undefined` is `true`,
        # every element counts as re-aimed, the wdio run PASSES and its manifest publishes a false
        # number. A missing field that makes a spec throw is a bad afternoon; this one is a green run
        # that reports something untrue about the product.
        "a renamed key inside the ELEMENT of a `.map(|x| json!({..})).collect()`",
        r"`p`\.marks\[0\]\.asDirected, but `json!@entity_probe\.marks\[\]` never sends "
        r"`asDirected`",
        {"overrides": with_probe(PROBE_FN.replace('"asDirected": m.ok,', '"asDirectedNow": m.ok,'))},
        {"overrides": with_probe()},
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
    # ── the same reads check, one level of INDIRECTION out ────────────────────────────────────────
    #
    # Proved on the committed tree before either case was written (2026-08-25, ADR-140): renaming
    # `camera_probe`'s top-level `shotPlacements` key was caught, and renaming `asDirected` INSIDE
    # its elements — read through `const placements = completedCamera.shotPlacements ?? [];` — was
    # `0 blocking`. A sub-path given a name of its own stopped being part of the reply, and the E2E
    # house style gives sub-paths names constantly.
    #
    # The second case is the real one: the alias is a LIST, and the drifted field is read through a
    # callback's element parameter, so it exercises the alias rule and the element rule composed.
    # Nothing composed them before, and each was individually correct.
    (
        "a renamed field read through a name given to a sub-object of the reply",
        r"is read as `seen`\.placement\.worldX, but `Placement` never sends `worldX`",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       swap(BASE_MAIN, "pub world_x: f64,", "pub easting: f64,")
                       .replace("world_x: 0.0,", "easting: 0.0,")}},
        {},
    ),
    (
        "a renamed field read through a callback element of an ALIASED list",
        r"is read as `seen`\.tags\[0\]\.asDirected, but `Tag` never sends `asDirected`",
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       swap(BASE_MAIN, "pub as_directed: bool,", "pub filmed_as_directed: bool,")}},
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


def _json_entries_skipping(inner: str) -> list | None:
    """`_json_entries` as it would be if an unreadable entry were SKIPPED rather than fatal.

    The tempting version: read what you can, ignore what you cannot. It produces an object that
    claims to carry fewer keys than it does, and `_read_findings` calls a key absent on exactly that
    evidence. Reach bought with false findings is not reach.
    """
    entries = []
    for entry in rustipc.split_top(inner):
        km = re.match(r'^"([^"]*)"\s*:', entry.strip())
        if not km:
            continue
        entries.append((km.group(1), *rustipc._json_value_shape(entry.strip()[km.end():])))
    return entries or None


def _old_return_type(src: str, i: int) -> str:
    """The original: a regex stopping at `{` OR `;`. `-> [f64; 8]` contains a semicolon."""
    m = re.match(r"\s*->\s*([^{;]+)\{", src[i:])
    return " ".join(m.group(1).split()) if m else "()"


def _old_angle(src: str, i: int) -> bool:
    """The original: every `<`/`>` counted as a bracket, including the `>` in a match arm's `=>`."""
    return True


def _arg_struct_by_bare_name(rs, ty: str):
    """`_arg_struct` as it was first written: the last `::` segment, looked up by name.

    This is the real `Composition` bug, reduced. `compose(composition: metrocalk_core::compose::
    Composition)` takes the struct in `core/src/compose.rs`; the bare-name winner is the unrelated
    one in `core/src/variant.rs`, and comparing the caller against it reported `id` and `nodes`
    "never sent" and `ops` "not accepted" — three confident findings about correct code, which is
    the failure mode that gets a gate waived rather than read.
    """
    base, _ = audit._peel(ty)
    bare = re.sub(r"<.*", "", base.split("::")[-1]).strip()
    if not re.fullmatch(r"[A-Za-z_]\w*", bare):
        return None, ""
    if bare in audit._ARG_SCALARS or bare in audit._ARG_INJECTED:
        return None, ""
    d = rs.arg_dtos.get(bare)
    return (d, "") if d else (None, "")


#: `#[serde(default = "path")]` only — the bare `#[serde(default)]` form missed. A field serde fills
#: in is then read as one the caller must send, and the tool reports correct code as a drift on every
#: run. This is the UNDER-suppression direction, which is the one that gets a gate turned off.
_DEFAULT_NEEDS_EQUALS = re.compile(r"(?:^|[\s,(])default(?=\s*=)")


def _binding_ignoring_scope(bindings, name: str, at: int):
    """The first annotation of that name anywhere in the file — "nearest declaration", not scope.

    `session.ts` binds `request` twice: once as a parameter of `remember`, whose body the call is not
    inside, and once as the parameter of the method that actually invokes. Reading the first compares
    a real payload struct against an unrelated reply type and reports every field of both.
    """
    hits = [b for b in bindings if b[0] == name]
    return hits[0][1] if hits else None



def _arg_struct_one_candidate_wins(rs, ty: str):
    """`_arg_struct` with the path consulted only to break a TIE — the first repair's own hole.

    Three Rust trees are swept, so an argument type from any other crate has no candidate of its own
    and a single unrelated struct sharing the bare name won the lookup unopposed. The docstring
    condemning bare-name resolution and the code doing it were four lines apart.
    """
    base, _ = audit._peel(ty)
    segs = [p for p in base.split("::") if p]
    bare = re.sub(r"<.*", "", segs[-1]).strip()
    if not re.fullmatch(r"[A-Za-z_]\w*", bare):
        return None, ""
    if bare in audit._ARG_SCALARS or bare in audit._ARG_INJECTED or bare in audit._ARG_OPAQUE:
        return None, ""
    cands = [(rel, d) for (rel, n), d in rs.arg_dtos_local.items() if n == bare]
    if not cands:
        return None, "none"
    if len(cands) == 1:
        return cands[0][1], ""
    keep = [(rel, d) for rel, d in cands if not audit._path_contradicts(segs[:-1], rel)]
    return (keep[0][1], "") if len(keep) == 1 else (None, "ambiguous")


def _no_extends(types) -> None:
    """`_fold_extends` as a no-op — an inherited field becomes an undeclared one."""
    for t in types.values():
        t.extends = ()


#: A `#[serde(alias = "..")]` reader that matches nothing, and a `rename(deserialize = "..")` reader
#: that matches nothing. Both are UNDER-suppression: correct code is reported as drifted, twice over
#: in each case, because the name the caller sends and the name the struct declares are then two
#: different keys and each is reported as missing from the other side.
_ALIAS_BLIND = re.compile(r"(?!x)x")
_RENAME_SPLIT_BLIND = re.compile(r"(?!x)x")

#: A `const`/`let` reader that only sees ANNOTATED declarations, so an unannotated inner binding
#: cannot shadow an outer typed one and the outer type is applied straight through it.
_UNANNOTATED_BLIND = re.compile(r"(?!x)x")

READER_MUTATIONS: list[tuple[str, str, str, object, dict]] = [
    (
        "an inherited field is not a declared one, if `extends` is thrown away",
        r"`filter\.kindList` is required by `MarkFilter`",
        "tsipc._fold_extends",
        _no_extends,
        {},
    ),
    (
        "the path consulted only to break a tie leaves a type from an unswept crate unopposed",
        r"`request\.decoy_only` is required by `MarkRequest` \(core/src/lib\.rs",
        "audit._arg_struct",
        _arg_struct_one_candidate_wins,
        {"overrides": {"editor-shell/src-tauri/src/main.rs":
                       BASE_MAIN.replace("request: marks::MarkRequest",
                                         "request: metrocalk_wire::proto::MarkRequest")},
         "drop": {"editor-shell/src/marks.rs"}},
    ),
    (
        "a #[serde(alias)] missed reports the alias AND the field it aliases, in opposite directions",
        r"`filter\.kindList` is required by `MarkFilter`",
        "rustipc._ALIAS",
        _ALIAS_BLIND,
        {},
    ),
    (
        "the split rename(deserialize = ..) form missed compares the field under its Rust name",
        r"`filter\.query` is required by `MarkFilter`",
        "rustipc._RENAME_SPLIT",
        _RENAME_SPLIT_BLIND,
        {},
    ),
    (
        "an unannotated inner binding that cannot shadow lets an outer type through",
        r"the caller declares `request\.parentId`.*has no such field",
        "tsipc._UNANNOTATED_LOCAL",
        _UNANNOTATED_BLIND,
        {},
    ),
    (
        "an argument type resolved by bare name picks the wrong struct of that name",
        r"`request\.decoy_only` is required by `MarkRequest` \(core/src/lib\.rs",
        "audit._arg_struct",
        _arg_struct_by_bare_name,
        {},
    ),
    (
        "a bare #[serde(default)] missed makes a field the caller may omit look required",
        r"`request\.pinned` is required by `MarkRequest`",
        "rustipc._SERDE_DEFAULT",
        _DEFAULT_NEEDS_EQUALS,
        {},
    ),
    (
        "an argument value's type read from the nearest binding rather than the enclosing scope",
        r"declares `request\.parentId`.*has no such field",
        "tsipc.binding_type",
        _binding_ignoring_scope,
        {},
    ),
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
        # The all-or-nothing rule, mutated into the shape it exists to prevent. `_json_entries`
        # returns None the moment ONE entry defeats it; a version that skipped the entry instead
        # would report every key of a partially-read object as absent — fourteen confident findings
        # about correct code, in a file no type-checker opens. Measured on the real tree while this
        # was being built: making one `capabilities` key an expression takes the live finding from
        # 1 blocking to 0, which is the honest direction to fail in.
        "a json! object read PARTLY, when one entry defeats the reader, invents absent keys",
        r"`json!@entity_probe\.where` never sends `wired`",
        "rustipc._json_entries",
        _json_entries_skipping,
        # The entry that defeats the reader is the one the spec READS, which is the only arrangement
        # that makes the two behaviours differ: skipping it enumerates `where` without `wired` and
        # reports it absent, while refusing the object whole reports nothing at all.
        {"overrides": with_probe(PROBE_FN.replace('"wired": true,', "WIRED_KEY: true,"))},
    ),
    (
        # The attribution rule. A read inside a function that takes the tracked name as a PARAMETER
        # is a read of the parameter; attributing it to the reply names a real file and a real line
        # about code that is correct, which is the one thing that gets a gate waived. Live in the
        # tree today at `factory-cinematic.e2e.js:1264` — `tracks.filter(({ profile }) =>
        # profile.cycle === "revolve")`, eighty lines below `const profile = await
        # invoke("set_render_profile", ..)`. It stays quiet there only because that command returns
        # a `String` and the walk stops on "not a struct". It would not stay quiet on a struct.
        "a name rebound as a function parameter, still attributed to the reply",
        r"`p`\.tally, but `json!@entity_probe` never sends `tally`",
        "jsreads.shadow_spans",
        lambda src, name, start, end: [],
        {"overrides": {
            **with_probe(),
            "editor-shell/e2e/specs-probe/probe.e2e.js": PROBE_E2E.replace(
                "  });\n});\n",
                "    const rows = [{ tally: 1 }];\n"
                "    manifest.n = { count: rows.filter(({ p }) => p.tally > 0).length };\n"
                "  });\n});\n",
            ),
        }},
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


#: `(name, spec source, the exact field paths the reader must recover)`.
#:
#: Each pin holds ONE condition of the alias rule, and each is written so that removing the condition
#: changes the recovered set — which is the only observable a silent failure leaves.
ALIAS_PATH_PINS = [
    (
        "an alias of a sub-object carries the sub-path into every read made through it",
        'const r = await invoke("c");\nconst p = r.placement;\nif (p.label) x();\n',
        ["placement", "placement.label"],
    ),
    (
        "an alias defaulted with an empty literal is still the sub-path, through a callback element",
        'const r = await invoke("c");\nconst xs = r.tags ?? [];\nif (xs.some((t) => t.flag)) x();\n',
        ["tags", "tags.[].flag"],
    ),
    (
        "a chain ending in a builtin binds the BUILTIN's result, and its reads are not the list's",
        # A PROPERTY builtin (`.length`), not a method call, and this pin was rewritten once for
        # exactly that reason: written as `r.tags.at(0)` it passed while the guard it names was
        # deleted, because the trailing `(0)` is a tail the NEXT condition already rejects. Two
        # conditions, one observable, and the pin was holding the wrong one. `.length` leaves the
        # initializer ending where the chain ends, so this is the only condition left to stop it.
        # The read off `count` is synthetic — the realistic forms of this mistake read nothing at
        # all, and a pin that recovers an empty set cannot tell a guard from a broken parser.
        'const r = await invoke("c");\nconst count = r.tags.length;\nif (count.flag) x();\n',
        ["tags"],
    ),
    (
        "a default that is another expression leaves a shape this reader has never seen",
        'const r = await invoke("c");\nconst p = r.placement ?? other;\nif (p.label) x();\n',
        ["placement"],
    ),
    (
        "an initializer that does not end at the chain is not the sub-path",
        'const r = await invoke("c");\nconst s = r.placement.label + "!";\nif (s.label) x();\n',
        ["placement.label"],
    ),
    (
        "a bare reassignment is not an alias: its prior meaning is not this reader's to reason about",
        'const r = await invoke("c");\nlet p;\np = r.placement;\nif (p.label) x();\n',
        ["placement"],
    ),
]

#: `(name, the body of a `json!` command, the shapes the reader must recover)`.
#:
#: The nested-`json!` rule has conditions that no verdict-shaped case can hold, for the same reason
#: two of the alias rule's three cannot: violating them does not produce a wrong FINDING, it
#: produces a shape the walk cannot resolve, and stops. A run that refused an ambiguous value and a
#: run whose reader is broken print the same silence. So the recovered shapes are asserted directly:
#: `name -> sorted keys`, with an absent name meaning "this reader proved nothing about it", which
#: is the honest outcome for four of these six.
JSON_SHAPE_PINS = [
    (
        "a nested object literal is enumerated as a shape of its own",
        '{ "a": 1, "b": { "c": 2, "d": 3 } }',
        {"json!@probe": ["a", "b"], "json!@probe.b": ["c", "d"]},
    ),
    (
        "a `.map(|x| json!({..})).collect()` is enumerated as the shape of its ELEMENTS",
        '{ "xs": v.iter().map(|x| serde_json::json!({ "p": x.p, "q": x.q })).collect::<Vec<_>>() }',
        {"json!@probe": ["xs"], "json!@probe.xs[]": ["p", "q"]},
    ),
    (
        # An iterator is not an array and cannot be one: serde has nothing to serialise it as. If
        # the `.collect()` requirement were dropped this would resolve to a list that the reply
        # never contains, and every read through it would be judged against an invented shape.
        "a `.map()` that never collects is not a list, and is not resolved",
        '{ "xs": v.iter().map(|x| serde_json::json!({ "p": x.p })) }',
        {"json!@probe": ["xs"]},
    ),
    (
        # Two macros mean a conditional, a nested map, or a shape with more than one reading. There
        # is no honest way to pick one, and picking one is the confident-wrong-answer class.
        #
        # This pin was REWRITTEN because it passed its own mutant. Written as a bare
        # `if c { json!(..) } else { json!(..) }` it stayed green while the one-macro rule was
        # deleted — because that value carries no `.collect()` tail, so the branch being mutated was
        # never reached and the pin was holding the NEXT condition instead. The two macros have to
        # sit inside a chain that would otherwise resolve, or the case proves nothing.
        "a value carrying TWO json! literals has more than one shape, so it has none here",
        '{ "xs": v.iter().map(|x| if x.ok { serde_json::json!({ "p": 1 }) } '
        'else { serde_json::json!({ "q": 2 }) }).collect::<Vec<_>>() }',
        {"json!@probe": ["xs"]},
    ),
    (
        # No binding resolution, deliberately. `spaces` is built ten lines above in the real
        # `colour_status`, and following a name to its initializer means deciding it was never
        # reassigned in between — a judgement this reader has no basis for.
        "a value that is a NAME is not followed to whatever built it",
        '{ "xs": spaces }',
        {"json!@probe": ["xs"]},
    ),
    (
        "a computed key defeats the object it is in, and only that object",
        '{ "a": 1, "b": { KEY: 2, "d": 3 } }',
        {"json!@probe": ["a", "b"]},
    ),
    (
        # Found by an adversarial re-read of this change, not by a case. The tail test only looks at
        # what FOLLOWS the macro, so an early `return json!({..})` is a second shape sitting where it
        # cannot see. Nothing in the swept trees writes one; the docstring claimed it was handled,
        # which is the whole reason to pin it — an unenforced claim decays into a wrong one.
        "an early `return json!` is a second shape, so the tail literal is not the contract",
        None,  # a whole body rather than one literal; see the runner
        {},
    ),
    (
        "a key written twice is ONE key on the wire, and is listed once",
        '{ "a": 1, "a": 2, "b": 3 }',
        {"json!@probe": ["a", "b"]},
    ),
]

#: `(name, a parameter list, the names it BINDS)`. `shadow_spans` suppresses a read on this answer,
#: so a pattern this gets wrong is a read silently attributed to the wrong reply (too few names) or
#: silently dropped (too many). Both are invisible in the verdict — hence a value pin. The first two
#: are regressions found by re-reading this change adversarially rather than by running it.
PATTERN_BIND_PINS = [
    ("a default inside a destructuring must not cut the pattern in half",
     "{ profile = 1 }", ["profile"]),
    ("a rest element binds its own name", "{ ...profile }", ["profile"]),
    ("a pattern may carry a default of its own", "{ a, profile } = {}", ["a", "profile"]),
    ("`key: target` binds the TARGET, and leaves the key meaning what it meant outside",
     "{ profile: p }", ["p"]),
    ("a nested pattern is walked, not skipped", "{ outer: { profile } }", ["profile"]),
    ("an array pattern binds its elements", "[first, profile]", ["first", "profile"]),
    ("a plain parameter with a default binds the name, not the default",
     "profile = fallback", ["profile"]),
]

#: `(name, spec source, the exact field paths the reader must recover)` — the shadow rule, held the
#: same way and for the same reason. Suppressing a read that should have been attributed and
#: attributing one that should have been suppressed are both silent: the first shows up as a number
#: nobody diffs, the second as a finding that looks exactly like a real one.
SHADOW_PATH_PINS = [
    (
        "a destructured parameter one brace deeper rebinds the name, and its reads are not the reply's",
        'const p = await invoke("c");\nif (p.mode) x();\n'
        "m.a = { n: rows.filter(({ p }) => p.tally > 0).length };\n",
        ["mode"],
    ),
    (
        # The other direction, and the reason the pattern is walked rather than grepped: `{ p: q }`
        # binds `q`. A reader that suppressed on any name APPEARING in the parameter list would drop
        # this read, and dropping reads is how a gate quietly stops covering things.
        "`{ p: q }` binds `q`, so the outer `p` still means the reply",
        'const p = await invoke("c");\n'
        "m.a = { n: rows.filter(({ p: q }) => q.ok && p.mode).length };\n",
        ["mode"],
    ),
    (
        "a plain parameter one brace deeper shadows just as a destructured one does",
        'const p = await invoke("c");\nif (p.mode) x();\n'
        "m.a = { n: rows.filter((p) => p.tally > 0).length };\n",
        ["mode"],
    ),
    (
        "a `function` declaration's parameter shadows too",
        'const p = await invoke("c");\nif (p.mode) x();\n'
        "m.a = { f: function scan(p) { return p.tally; } };\n",
        ["mode"],
    ),
    (
        # The shape of the rule, stated as a value: a shadow is a HOLE in the binding's scope, not
        # an end to it. `_scope_end` truncates at a shadow only at the binding's own brace depth,
        # where truncating and holing are the same thing; one brace deeper they are not, and reads
        # after the closure still belong to the reply.
        #
        # Also REWRITTEN after passing its own mutant. The later read has to sit inside the same
        # enclosing block as the closure, or an over-broad span that runs to the end of that block
        # stops before reaching it and the pin never notices — which is precisely the version this
        # first had, with `m.a = {..}` and the read outside it.
        "a nested shadow is a hole in the scope, not an end to it: the reads after it still count",
        'const p = await invoke("c");\n'
        "if (ok) {\n  m.a = rows.filter(({ p }) => p.tally > 0).length;\n"
        "  if (p.mode) x();\n}\n",
        ["mode"],
    ),
]


# ── pins that no verdict-shaped case can hold ─────────────────────────────────────────────────────
#
# Each of these guards a REFUSAL or a SPAN. A refusal and a broken reader produce identical silence,
# and a span that is too long produces a wrong answer only when some other binding happens to sit
# inside it — so a case can pass while the mechanism it names is deleted. Every one of these was
# written because an adversarial pass neutered the code in a single line and the suite stayed green.

#: (label, snippet, name, marker, expected type or None). The offset is taken at `marker`, which must
#: occur exactly once, so the pin reads like the source it is about.
BINDING_PINS: list[tuple[str, str, str, str, str | None]] = [
    (
        "a const does not reach a later SIBLING block — the span ends at its own brace",
        "function f() { { const a: Alpha = mk(); void a; } { HERE } }",
        "a", "HERE", None,
    ),
    (
        "a const DOES reach the rest of its own block",
        "function f() { const a: Alpha = mk(); HERE }",
        "a", "HERE", "Alpha",
    ),
    (
        "an unannotated inner binding shadows an annotated outer one, and declares nothing",
        "function f() { const a: Alpha = mk(); { const a = other(); HERE } }",
        "a", "HERE", None,
    ),
    (
        "a parameter of a function the call is not inside is not a statement about that call",
        "function g(a: Alpha) { void a; }\nfunction f() { HERE }",
        "a", "HERE", None,
    ),
    (
        "an interface member with no trailing semicolon does not steal the next function's body",
        "interface Api {\n  m(a: Alpha): Promise<void>\n}\nfunction f(a: Beta) { HERE }",
        "a", "HERE", "Beta",
    ),
    (
        "an UNANNOTATED parameter shadows too",
        "function g(a: Alpha) { function h(a) { HERE } }",
        "a", "HERE", None,
    ),
]

#: (label, Rust type expression, what `_arg_struct` must do with it). "open" means it resolves to a
#: struct; "skip" means it is not a struct and is not a gap either — the distinction the header's
#: numbers rest on, and the one a deleted `_ARG_SCALARS` or a deleted enum guard silently erases.
ARG_RESOLVE_PINS: list[tuple[str, str, str]] = [
    ("a payload struct opens", "marks::MarkRequest", "open"),
    ("a scalar is not a struct and not a gap", "String", "skip"),
    ("a fixed-size array has no field names", "[f64; 3]", "skip"),
    ("a map keyed at run time has none either", "BTreeMap<String, serde_json::Value>", "skip"),
    ("an injected handle is not part of the payload", "State<AppState>", "skip"),
    # `Channel<T>` IS a key the caller sends — `arguments` checks that it is sent — and it has no
    # field names inside it. "Not injected" and "nothing to open" are different statements, and an
    # earlier comment made the first one while meaning the second.
    ("a channel is sent by the caller and still has nothing to open", "Channel<Delta>", "skip"),
    ("an enum argument is a variant set, which `variants` compares", "Facing", "skip"),
    ("a name no swept file corroborates is a GAP, not a resolution",
     "metrocalk_wire::proto::MarkRequest", "gap"),
    ("a crate segment corroborates through its directory", "metrocalk_core::Facing", "skip"),
]


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

    # A mutation names the module it patches, because the reader is no longer one file: the Rust
    # parser, the JavaScript reader and the comparison layer can each hold a wrong answer, and all
    # three are worth pinning.
    modules = {"rustipc": rustipc, "audit": audit, "jsreads": jsreads, "tsipc": tsipc}

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

    # ── the alias rule, pinned by VALUE rather than by verdict ────────────────────────────────────
    #
    # Two of the rule's three conditions cannot be shown with a fixture case, and saying so is the
    # point. Their violation does not produce a WRONG FINDING that a case could catch — it produces
    # a path the comparison layer then cannot resolve, and stops. A run that followed a bad alias and
    # a run that followed nothing print the same silence, so the only way to hold these is to assert
    # the recovered paths themselves. (The GPU audit's string-continuation decoder is pinned to its
    # value for exactly this reason: no verdict-shaped case can see it.)
    for label, pins in (("alias", ALIAS_PATH_PINS), ("shadow", SHADOW_PATH_PINS)):
        for name, snippet, expected in pins:
            got = sorted(".".join(rd.path) for rd in jsreads.parse(snippet, "pin.e2e.js").reads)
            if got != sorted(expected):
                print(f"FAIL  the {label} rule recovers the wrong paths: {name}")
                print(f"        expected {sorted(expected)}")
                print(f"        got      {got}")
                ok = False
            else:
                print(f"pass  {name}")

    for name, params, expected in PATTERN_BIND_PINS:
        got = sorted(jsreads._pattern_binds(params))
        if got != sorted(expected):
            print(f"FAIL  a parameter pattern binds the wrong names: {name}")
            print(f"        `{params}` -> expected {sorted(expected)}, got {got}")
            ok = False
        else:
            print(f"pass  {name}")

    # The `json!` shape reader, pinned the same way: most of its conditions are refusals, and a
    # refusal and a broken parser produce identical output.
    for name, body, expected in JSON_SHAPE_PINS:
        inner = (
            f"    serde_json::json!({body})\n"
            if body is not None
            # The one pin whose subject is the BODY rather than the literal: two shapes, one of them
            # before the tail. A literal alone cannot express it.
            else '    if early { return serde_json::json!({ "err": 1 }); }\n'
                 '    serde_json::json!({ "ok": 2 })\n'
        )
        src = (
            "#[tauri::command]\nfn probe() -> serde_json::Value {\n"
            f"{inner}}}\n"
            "fn main() { tauri::Builder::default()"
            ".invoke_handler(tauri::generate_handler![probe]); }\n"
        )
        rs = rustipc.parse([("editor-shell/src-tauri/src/main.rs", src)])
        got = {
            n: sorted(k for k, _ in d.fields)
            for n, d in rs.dtos.items()
            if d.origin == "json!"
        }
        if got != {k: sorted(v) for k, v in expected.items()}:
            print(f"FAIL  the json! reader recovers the wrong shapes: {name}")
            print(f"        expected {expected}")
            print(f"        got      {got}")
            ok = False
        else:
            print(f"pass  {name}")

    # ── the two universes, pinned by VALUE ────────────────────────────────────────────────────────
    #
    # `Serialize` says what the shell can SEND and `Deserialize` says what a command can READ, and
    # they are not the same set of structs. No verdict-shaped case can hold this: if the argument map
    # wrongly contained a send-only struct, or the reply map a read-only one, the result is a lookup
    # answering out of the wrong population — which usually produces the same silence as a lookup
    # that found nothing, and occasionally a confident finding about a wire shape that cannot occur.
    # `Serialize` is also a SUBSTRING of `Deserialize`, so the test that separates them is the one
    # thing here most likely to be rewritten into an `in` and pass every case above.
    with tempfile.TemporaryDirectory() as tmp:
        _rs = rustipc.parse(audit.sweep_rust(build(tmp)))
    for label, name, in_dtos, in_args in (
        ("a Deserialize-only payload is an argument struct and never a reply one",
         "MarkRequest", False, True),
        ("a Serialize-only DTO is a reply struct and never an argument one",
         "EntityInfo", True, False),
        # A struct deriving BOTH belongs to both universes — asserted inside the `MarkRequest`
        # branch below, where the decoy is the both-deriving one. A row here naming a
        # Serialize-only struct and calling it "both" would be a label the assertion does not
        # carry, which is how a pin comes to prove something other than what it says.
    ):
        got = (name in _rs.dtos, name in _rs.arg_dtos)
        if name == "MarkRequest":
            # `dtos` must hold the DECOY (which derives both) and not the payload — so ask the map
            # that carries the file, not the one that carries the name.
            got = (_rs.dtos["MarkRequest"].file == "core/src/lib.rs",
                   _rs.arg_dtos["MarkRequest"].file == "core/src/lib.rs")
            if got != (True, True):
                print(f"FAIL  {label}: bare-name winners moved ({got})")
                ok = False
            else:
                d = _rs.arg_dtos_local[("editor-shell/src/marks.rs", "MarkRequest")]
                if ("editor-shell/src/marks.rs", "MarkRequest") in _rs.dtos_local:
                    print("FAIL  a Deserialize-only struct reached the REPLY map")
                    ok = False
                elif sorted(f for f, _ in d.fields) != ["atFrame", "label", "note", "pinned"]:
                    print(f"FAIL  the argument struct's wire keys are wrong: {d.fields}")
                    ok = False
                else:
                    print(f"pass  {label}")
            continue
        if got != (in_dtos, in_args):
            print(f"FAIL  {label}: in dtos={got[0]}, in arg_dtos={got[1]}")
            ok = False
        else:
            print(f"pass  {label}")

    for label, snippet, name, marker, want in BINDING_PINS:
        assert snippet.count(marker) == 1, f"marker must be unique: {label}"
        at = snippet.index(marker)
        got = tsipc.binding_type(tsipc.annotated_bindings(snippet), name, at)
        if got != want:
            print(f"FAIL  a binding resolves wrongly: {label}")
            print(f"        expected {want!r}, got {got!r}")
            ok = False
        else:
            print(f"pass  {label}")

    with tempfile.TemporaryDirectory() as tmp:
        _prs = rustipc.parse(audit.sweep_rust(build(tmp)))
    for label, ty, want in ARG_RESOLVE_PINS:
        dto, why = audit._arg_struct(_prs, ty)
        got = "open" if dto is not None else ("gap" if why else "skip")
        if got != want:
            print(f"FAIL  an argument type resolves wrongly: {label}")
            print(f"        `{ty}` -> expected {want}, got {got} ({why or 'no reason'})")
            ok = False
        else:
            print(f"pass  {label}")

    # A struct carrying `#[serde(flatten)]` splices in keys this reader cannot enumerate, so neither
    # "the caller omitted a required field" nor "the caller sent one the struct has no room for" is
    # provable. The refusal is invisible to a case — it produces the same nothing a clean tree does —
    # so it is held here, by the count: the key must NOT be opened.
    with tempfile.TemporaryDirectory() as tmp:
        flat = audit.run(build(tmp, {"editor-shell/src/marks.rs": swap(
            MARKS_RS, "    pub note: Option<String>,",
            "    #[serde(flatten)]\n    pub extra: MarkExtra,")}))
    if flat[0]:
        print("FAIL  a flattened argument struct produced findings instead of a refusal:")
        for f in flat[0]:
            print(f"        [{f.check}] {f.message[:130]}")
        ok = False
    else:
        with tempfile.TemporaryDirectory() as tmp:
            whole = audit.run(build(tmp))[1]["argfields_keys"]
        if flat[1]["argfields_keys"] != whole - 1:
            print(f"FAIL  a flattened argument struct was still opened "
                  f"({flat[1]['argfields_keys']} key(s) opened, expected {whole - 1} — every "
                  f"argument key in the fixture but `entity_mark`'s)")
            ok = False
        else:
            print("pass  a flattened argument struct is refused, not compared")

    # ── the header's own numbers, pinned as SETS ───────────────────────────────────────────────────
    #
    # `opened` and `untyped` describe disjoint parts of ONE population, and the first version counted
    # `opened` per CALL SITE instead of per (command, key) — which made the header read
    # `3 of 1 argument key(s) ... opened` the moment a payload command gained a second caller. The
    # fixture has that second caller (`entityMarkAlias` passes `entity_mark`'s key untyped), so the
    # two numbers can disagree here, and nothing else in this file would notice if they did: a wrong
    # number is not a finding, and every case above would stay green beside it.
    with tempfile.TemporaryDirectory() as tmp:
        _st = audit.run(build(tmp))[1]
    _tot, _open, _un = _st["arg_struct_keys"], _st["argfields_keys"], _st["argfields_unresolved"]
    if _open + _un > _tot:
        print(f"FAIL  the header's argument numbers do not describe one population: "
              f"{_open} opened + {_un} untyped > {_tot} keys")
        ok = False
    elif (_tot, _open, _un) != (4, 3, 1):
        print(f"FAIL  the fixture's argument reach moved: expected 4 keys / 3 opened / 1 untyped, "
              f"got {_tot} / {_open} / {_un}")
        ok = False
    else:
        print("pass  opened and untyped are disjoint parts of one population, counted per key")

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
